//! CPU execution of the small integer control-flow modules used by lowering tests.

use super::*;

pub(crate) struct Execution {
    pub blocks: Vec<usize>,
    pub values: HashMap<V, u32>,
    pub stores: Vec<(V, u32)>,
    pub discarded: bool,
}

fn word(m: &Module, values: &HashMap<V, u32>, v: V) -> u32 {
    match v {
        V::Const(i) => match m.consts[i as usize].1 {
            Const::Int(n) => n as u32,
            _ => panic!("expected integer constant"),
        },
        _ => *values.get(&v).expect("value must dominate its use"),
    }
}

/// Check every phi against the actual predecessor blocks, including edges not executed.
pub(crate) fn check_phis(m: &Module) {
    for (b, block) in m.blocks.iter().enumerate() {
        let mut predecessors: Vec<_> = m
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(from, block)| successors(block.term).contains(&b).then_some(from))
            .collect();
        predecessors.sort_unstable();
        for &id in &block.phis {
            let Op::Phi(incoming) = &m.insts[id as usize].op else {
                panic!("not a phi");
            };
            let mut from: Vec<_> = incoming.iter().map(|(_, from)| *from).collect();
            from.sort_unstable();
            assert_eq!(from, predecessors, "phi {id} in block {b}");
        }
    }
}

/// Check the initialization and allocation emitted for an authored split-wave program.
pub(crate) fn check_wave_counter(m: &Module, base: V, barrier_mode: u32) {
    let op = |v| {
        let V::Inst(id) = v else {
            panic!("expected instruction")
        };
        &m.insts[id as usize].op
    };
    let constant = |v| word(m, &HashMap::new(), v);
    let call = |v, name: &str, opcode| {
        let Op::Call(f, args) = op(v) else {
            panic!("expected {name}")
        };
        assert_eq!(m.funcs[*f as usize].name, name);
        assert_eq!(constant(args[0]), opcode);
        &args[1..]
    };
    let Some(Term::CondBr(first, init, join)) = m.blocks[0].term else {
        panic!("the reset must be guarded at entry");
    };
    let Op::Cmp(pred::EQ, id, zero) = op(first) else {
        panic!("expected ID comparison")
    };
    assert_eq!(constant(*zero), 0);
    assert!(call(*id, "dx.op.flattenedThreadIdInGroup.i32", 96).is_empty());
    assert!(matches!(m.blocks[init].term, Some(Term::Br(to)) if to == join));
    assert!(
        matches!(m.blocks[join].term, Some(Term::Br(to)) if to != 0 && to != init && to != join)
    );
    let [ptr, reset] = m.blocks[init].body[..] else {
        panic!("expected just the counter reset")
    };
    let Op::Store(reset_ptr, reset_value) = op(V::Inst(reset)) else {
        panic!("expected reset store")
    };
    assert_eq!(*reset_ptr, V::Inst(ptr));
    assert_eq!(constant(*reset_value), 0);
    let Op::Gep(_, reset_lds, reset_index) = op(*reset_ptr) else {
        panic!("expected LDS pointer")
    };
    let [barrier, ..] = m.blocks[join].body[..] else {
        panic!("expected group barrier")
    };
    assert_eq!(
        constant(call(V::Inst(barrier), "dx.op.barrier", 80)[0]),
        barrier_mode
    );
    let ticket = call(base, "dx.op.waveReadLaneFirst.i32", 118)[0];
    let Op::AtomicRmw(Rmw::Add, ticket_ptr, amount) = op(ticket) else {
        panic!("expected atomic ticket")
    };
    let Op::Gep(_, ticket_lds, ticket_index) = op(*ticket_ptr) else {
        panic!("expected LDS pointer")
    };
    assert_eq!((ticket_lds, ticket_index), (reset_lds, reset_index));
    let V::Inst(ticket) = ticket else {
        unreachable!()
    };
    assert!(
        m.blocks[join].body[1..].contains(&ticket),
        "the ticket must follow the barrier"
    );
    for lane in 0..32 {
        assert_eq!(
            lane_value(m, *amount, lane, 0, 32),
            if lane == 0 { 32 } else { 0 }
        );
    }
    check_phis(m);
}

/// Interpret the emitted operations, not a second implementation of the lowering rules.
/// Unknown operations fail the test; a step budget catches missing loop guards.
pub(crate) fn run(m: &Module, budget: usize) -> Execution {
    let mut result = Execution {
        blocks: Vec::new(),
        values: HashMap::new(),
        stores: Vec::new(),
        discarded: false,
    };
    let (mut at, mut previous) = (0, None);
    for _ in 0..budget {
        result.blocks.push(at);
        let block = &m.blocks[at];
        // Phi assignments are simultaneous, including self-referential loop values.
        let phis: Vec<_> = block
            .phis
            .iter()
            .map(|&id| {
                let Op::Phi(incoming) = &m.insts[id as usize].op else {
                    panic!("not a phi");
                };
                let &(value, _) = incoming
                    .iter()
                    .find(|(_, from)| Some(*from) == previous)
                    .expect("phi must have an incoming value for the executed edge");
                (V::Inst(id), word(m, &result.values, value))
            })
            .collect();
        result.values.extend(phis);
        for &id in &block.body {
            let get = |v| word(m, &result.values, v);
            let value = match &m.insts[id as usize].op {
                Op::Bin(op, a, b) => match op {
                    Bin::Add => get(*a).wrapping_add(get(*b)),
                    Bin::Sub => get(*a).wrapping_sub(get(*b)),
                    Bin::And => get(*a) & get(*b),
                    Bin::Or => get(*a) | get(*b),
                    Bin::Xor => get(*a) ^ get(*b),
                    _ => panic!("unexpected operation {op:?}"),
                },
                Op::Cmp(op, a, b) => u32::from(match *op {
                    pred::EQ => get(*a) == get(*b),
                    pred::NE => get(*a) != get(*b),
                    pred::IULT => get(*a) < get(*b),
                    pred::IUGE => get(*a) >= get(*b),
                    pred::SLT => (get(*a) as i32) < (get(*b) as i32),
                    _ => panic!("unexpected comparison {op}"),
                }),
                Op::Select(c, t, f) => get(if get(*c) != 0 { *t } else { *f }),
                Op::Store(ptr, value) => {
                    result.stores.push((*ptr, get(*value)));
                    0
                }
                Op::Call(f, args) if m.funcs[*f as usize].name == "dx.op.discard" => {
                    result.discarded |= get(args[1]) != 0;
                    0
                }
                op => panic!("unexpected instruction {op:?}"),
            };
            result.values.insert(V::Inst(id), value);
        }
        let next = match block.term.expect("terminated block") {
            Term::Br(next) => next,
            Term::CondBr(condition, yes, no) => {
                if word(m, &result.values, condition) != 0 {
                    yes
                } else {
                    no
                }
            }
            Term::Ret => return result,
        };
        previous = Some(at);
        at = next;
    }
    panic!("emitted control flow did not return within {budget} blocks");
}

/// Evaluate one emitted integer value for an authored lane-local fixture. Only
/// the entry wave-allocation atomic is modeled: its returned base identifies
/// this physical wave. This checks emitted data dependencies, not GPU barriers.
pub(crate) fn lane_value(m: &Module, value: V, lane: u32, base: u32, width: u32) -> u32 {
    lane_value_with_lds(m, value, lane, base, width, &HashMap::new())
}

fn lane_value_with_lds(
    m: &Module,
    value: V,
    lane: u32,
    base: u32,
    width: u32,
    memory: &HashMap<(V, u32), u32>,
) -> u32 {
    fn eval(
        m: &Module,
        value: V,
        lane: u32,
        base: u32,
        width: u32,
        memory: &HashMap<(V, u32), u32>,
        left: usize,
    ) -> u32 {
        assert!(left != 0, "lane expression exceeds its evaluation budget");
        let get = |v| eval(m, v, lane, base, width, memory, left - 1);
        let V::Inst(id) = value else {
            return word(m, &HashMap::new(), value);
        };
        match &m.insts[id as usize].op {
            Op::Bin(op, a, b) => match op {
                Bin::Add => get(*a).wrapping_add(get(*b)),
                Bin::Sub => get(*a).wrapping_sub(get(*b)),
                Bin::Mul => get(*a).wrapping_mul(get(*b)),
                Bin::And => get(*a) & get(*b),
                Bin::Or => get(*a) | get(*b),
                Bin::Xor => get(*a) ^ get(*b),
                Bin::Shl => get(*a).wrapping_shl(get(*b)),
                Bin::LShr => get(*a).wrapping_shr(get(*b)),
                Bin::UDiv => get(*a) / get(*b),
                Bin::URem => get(*a) % get(*b),
                _ => panic!("unexpected lane operation {op:?}"),
            },
            Op::Cmp(op, a, b) => u32::from(match *op {
                pred::EQ => get(*a) == get(*b),
                pred::NE => get(*a) != get(*b),
                pred::IULT => get(*a) < get(*b),
                pred::IUGE => get(*a) >= get(*b),
                _ => panic!("unexpected lane comparison {op}"),
            }),
            Op::Select(c, t, f) => get(if get(*c) != 0 { *t } else { *f }),
            Op::Cast(Cast::Bitcast | Cast::ZExt | Cast::Trunc, v) => get(*v),
            Op::Load(ptr) => {
                let V::Inst(id) = ptr else {
                    panic!("expected LDS GEP")
                };
                let Op::Gep(_, global, indices) = &m.insts[*id as usize].op else {
                    panic!("expected LDS GEP")
                };
                let index = get(*indices.last().unwrap());
                *memory
                    .get(&(*global, index))
                    .expect("LDS value must be published")
            }
            Op::AtomicRmw(Rmw::Add, _, amount) => {
                assert_eq!(lane, 0, "only wave-first allocation result is observed");
                assert_eq!(get(*amount), 32);
                base
            }
            Op::Call(f, args) => match m.funcs[*f as usize].name.as_str() {
                "dx.op.waveGetLaneIndex" => lane,
                "dx.op.waveIsFirstLane" => u32::from(lane == 0),
                "dx.op.flattenedThreadIdInGroup.i32" => base + lane,
                "dx.op.waveReadLaneFirst.i32" => eval(m, args[1], 0, base, width, memory, left - 1),
                "dx.op.waveReadLaneAt.i32" => {
                    let source = get(args[2]);
                    assert!(
                        source < width,
                        "shuffle source must stay within physical wave"
                    );
                    eval(m, args[1], source, base, width, memory, left - 1)
                }
                "dx.op.waveActiveBit.i32" => {
                    assert_eq!(get(args[0]), 120);
                    assert_eq!(get(args[2]), 1, "authored reduction is bitwise OR");
                    (0..width).fold(0, |result, source| {
                        result | eval(m, args[1], source, base, width, memory, left - 1)
                    })
                }
                name => panic!("unexpected lane call {name}"),
            },
            op => panic!("unexpected lane expression {op:?}"),
        }
    }
    eval(m, value, lane, base, width, memory, 512)
}

/// Inspect the actual emitted mailbox diamond and evaluate its two different half-results.
/// This is a CPU data-dependency/CFG check, not a model of GPU scheduling or memory ordering.
pub(crate) fn check_wave_mailboxes(m: &Module, output: V, barrier_mode: u32) {
    check_phis(m);
    let constant = |v| word(m, &HashMap::new(), v);
    let stores: Vec<_> = m
        .blocks
        .iter()
        .enumerate()
        .flat_map(|(block, b)| {
            b.body
                .iter()
                .filter_map(move |&id| match m.insts[id as usize].op {
                    Op::Store(ptr, value) if m.int_value(value) != Some(0) => {
                        Some((block, id, ptr, value))
                    }
                    _ => None,
                })
        })
        .collect();
    assert_eq!(
        stores.len(),
        1,
        "one authored reduction must use one mailbox store"
    );
    let (store_block, store_id, ptr, mine) = stores[0];
    assert_eq!(
        m.blocks[store_block].body,
        [store_id],
        "only the mailbox store is conditional"
    );
    let Some(Term::Br(join)) = m.blocks[store_block].term else {
        panic!("mailbox writer must rejoin every lane")
    };
    let predecessors: Vec<_> = m
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(block, b)| successors(b.term).contains(&store_block).then_some(block))
        .collect();
    assert_eq!(predecessors.len(), 1);
    let Some(Term::CondBr(first, yes, no)) = m.blocks[predecessors[0]].term else {
        panic!("ordinary mailbox store must be guarded by the first active lane")
    };
    assert_eq!((yes, no), (store_block, join));
    let V::Inst(first) = first else {
        panic!("expected wave-first intrinsic")
    };
    let Op::Call(function, args) = &m.insts[first as usize].op else {
        panic!("expected wave-first intrinsic")
    };
    assert_eq!(m.funcs[*function as usize].name, "dx.op.waveIsFirstLane");
    assert_eq!(m.funcs[*function as usize].attr, Some(Attr::NoUnwind));
    assert_eq!(args.len(), 1);
    assert_eq!(constant(args[0]), 110);
    let body = &m.blocks[join].body;
    let barriers: Vec<_> = body
        .iter()
        .enumerate()
        .filter_map(|(at, &id)| {
            let Op::Call(function, args) = &m.insts[id as usize].op else {
                return None;
            };
            if m.funcs[*function as usize].name != "dx.op.barrier" {
                return None;
            }
            assert_eq!(constant(args[0]), 80);
            assert_eq!(constant(args[1]), barrier_mode);
            Some(at)
        })
        .collect();
    assert_eq!(
        barriers.len(),
        2,
        "both barriers must be in the unconditional join"
    );
    assert_eq!(
        barriers[0], 0,
        "the publication barrier must be the first join instruction"
    );
    let loads: Vec<_> = body
        .iter()
        .enumerate()
        .filter_map(|(at, &id)| matches!(m.insts[id as usize].op, Op::Load(_)).then_some(at))
        .collect();
    assert_eq!(loads.len(), 2);
    assert!(loads.iter().all(|&at| barriers[0] < at && at < barriers[1]));
    let V::Inst(ptr) = ptr else {
        panic!("expected mailbox GEP")
    };
    let Op::Gep(_, global, indices) = &m.insts[ptr as usize].op else {
        panic!("expected mailbox GEP")
    };
    let mut memory = HashMap::new();
    for (base, expected) in [(0, 0x0000_ffff), (32, 0xffff_0000)] {
        let value = lane_value(m, mine, 0, base, 32);
        assert_eq!(
            value, expected,
            "the two physical halves must publish distinct reductions"
        );
        let index = lane_value(m, *indices.last().unwrap(), 0, base, 32);
        assert!(
            memory.insert((*global, index), value).is_none(),
            "the halves need distinct slots"
        );
        for lane in 1..32 {
            assert_eq!(lane_value(m, mine, lane, base, 32), value);
        }
    }
    for thread in 0..64 {
        assert_eq!(
            lane_value_with_lds(m, output, thread % 32, thread & !31, 32, &memory),
            u32::MAX,
            "output lane {thread} must combine both halves"
        );
    }
}

/// Check the actual typed/legacy access diamonds, including their phi predecessors.
pub(crate) fn check_image_classes(m: &Module, loads: usize, stores: usize) {
    let mut counts = [0, 0];
    for (block, body) in m.blocks.iter().enumerate() {
        for &id in &body.body {
            let Op::Call(f, args) = &m.insts[id as usize].op else { continue };
            let name = m.funcs[*f as usize].name.as_str();
            let store = match name {
                "dx.op.textureLoad.i32" => false,
                "dx.op.textureStore.i32" => true,
                _ => continue,
            };
            counts[usize::from(store)] += 1;
            let (condition, alternate) = m.blocks.iter().find_map(|b| match b.term {
                Some(Term::CondBr(c, yes, no)) if yes == block => Some((c, no)),
                _ => None,
            }).expect("typed access must have its own selected branch");
            let V::Inst(c) = condition else { panic!("class must be read at bind time") };
            let Op::Cmp(pred::EQ, _, one) = m.insts[c as usize].op else {
                panic!("class selects only the exact UINT value");
            };
            assert_eq!(m.int_value(one), Some(1));
            let legacy = if store { "dx.op.textureStore.f32" } else { "dx.op.textureLoad.f32" };
            let calls: Vec<_> = m.blocks[alternate].body.iter().filter_map(|&id| {
                let Op::Call(f, a) = &m.insts[id as usize].op else { return None };
                (m.funcs[*f as usize].name == legacy).then_some(a)
            }).collect();
            assert_eq!(calls.len(), 1, "opposite branch must perform one legacy access");
            assert!(matches!((body.term, m.blocks[alternate].term),
                (Some(Term::Br(a)), Some(Term::Br(b))) if a == b));
            if store {
                assert_eq!(m.int_value(*args.last().unwrap()), Some(15), "typed UAV store mask");
                assert_eq!(m.int_value(args[5]).map(|v| v as u32), Some(0x8000_0000));
                assert_eq!(args[6..9], [args[5]; 3], "typed scalar store values");
                assert_eq!(m.int_value(*calls[0].last().unwrap()), Some(15));
            }
        }
    }
    assert_eq!(counts, [loads, stores]);
    check_phis(m);
}

/// Reproduce the one-component store in an otherwise identical authored module.
#[cfg(windows)]
pub(crate) fn force_incomplete_typed_store_mask(m: &mut Module) {
    let mut stores = m.insts.iter().enumerate().filter_map(|(index, inst)| {
        let Op::Call(function, args) = &inst.op else { return None; };
        (m.funcs[*function as usize].name == "dx.op.textureStore.i32").then(|| (index, args[5]))
    });
    let (index, x) = stores.next().expect("authored direct typed store");
    assert!(stores.next().is_none());
    let undef = m.undef(m.type_of(x));
    let mask = m.i8(1);
    let Op::Call(_, args) = &mut m.insts[index].op else { unreachable!() };
    args[6..9].fill(undef);
    args[9] = mask;
}

/// Evaluate the emitted table-index arithmetic for an authored static SMEM descriptor load.
pub(crate) fn table_index(m: &Module, value: V, table_word: u32) -> u32 {
    if let Some(n) = m.int_value(value) { return n as u32; }
    let V::Inst(id) = value else { panic!("expected table-index SSA") };
    let eval = |v| table_index(m, v, table_word);
    match &m.insts[id as usize].op {
        Op::Bin(Bin::Add, a, b) => eval(*a).wrapping_add(eval(*b)),
        Op::Bin(Bin::LShr, a, b) => eval(*a) >> eval(*b),
        Op::Extract(row, component) => {
            let V::Inst(row) = row else { panic!("user-data row") };
            let Op::Call(f, args) = &m.insts[*row as usize].op else { panic!("user data load") };
            assert_eq!(m.funcs[*f as usize].name, "dx.op.cbufferLoadLegacy.i32");
            assert_eq!(m.int_value(args[2]), Some(0));
            assert_eq!(*component, 0);
            table_word
        }
        other => panic!("unexpected table-index expression: {other:?}"),
    }
}

pub(crate) fn check_image_store_exec_guard(m: &Module) {
    let store_block = m.blocks.iter().position(|b| b.body.iter().any(|&id| {
        matches!(&m.insts[id as usize].op, Op::Call(f, _)
            if m.funcs[*f as usize].name == "dx.op.textureStore.i32")
    })).unwrap();
    let class_block = m.blocks.iter().position(|b|
        matches!(b.term, Some(Term::CondBr(_, yes, _)) if yes == store_block)).unwrap();
    assert!(m.blocks.iter().any(|b| matches!(b.term,
        Some(Term::CondBr(_, yes, no)) if yes == class_block && no != store_block)),
        "the numeric class branch must remain inside the original EXEC branch");
}
