//! A console wave64 compute program of two console waves on a host of 32-lane waves, which
//! writes LDS and reads another half's dword inside one per-lane branch: with
//! `LEONIDA_WAVE64_GROUP_WIDE=1` every thread takes that branch ([`forced_wide`]), so the
//! barriers before its LDS accesses meet every thread of the group. Authored code only.

use std::collections::HashSet;

use super::*;
use crate::emit::bitcode::execute;
use crate::emit::HostShader;
use crate::emit::host::{Dispatch, Host, Root, View};
use crate::emit::validator::Validator;
use crate::native::{ComputeIo, ResourceType, RootParam, RootType, UsedResource};
use crate::resource::Source;

const THREADS: u32 = 128;
/// The skip past the body: `s_cbranch_scc0` on the SCC `s_and_saveexec` sets (some lane is
/// left), which the IR form without skips (`ir::without_skips`, `s_cbranch_execz` only) keeps.
const SCC0: u32 = 0xBF84_0000;
const EXECZ: u32 = 0xBF88_0000;

/// Thread t (v0) of a group of 128: v1 = lane 32 of its console wave's thread index
/// (`v_readlane`); then, if t < s0, LDS dword t = t and v1 = LDS dword t ^ 32, which a lane of
/// the other half of its console wave wrote; v1 goes to dword t of the raw UAV.
/// `skip`: the branch past the body. `scalar_left`: the branch also writes s6, which code after
/// it reads.
fn native(skip: u32, scalar_left: bool) -> NativeProgram {
    let mut body = vec![
        0xD834_0000,
        0x0000_0003, // ds_write_b32 v3, v0
        0x3A04_00A0, // v_xor_b32 v2, 32, v0
        0x3404_0482, // v_lshlrev_b32 v2, 2, v2
        0xD8D8_0000,
        0x0100_0002, // ds_read_b32 v1, v2
        0xBF8C_0000, // s_waitcnt 0
    ];
    if scalar_left {
        body.push(0xBE86_0387); // s_mov_b32 s6, 7
    }
    let mut words = vec![
        0xBE85_0380, // s_mov_b32 s5, 0
        0xF408_0202,
        0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], 0
        0x3406_0082, // v_lshlrev_b32 v3, 2, v0
        0xD760_0006,
        0x0001_4100, // v_readlane_b32 s6, v0, 32
        0x7E02_0206, // v_mov_b32 v1, s6
        0x7D88_0000, // v_cmp_gt_u32 vcc, s0, v0
        0xBE8C_246A, // s_and_saveexec_b64 s[12:13], vcc
        skip | body.len() as u32, // the skip past the body
    ];
    words.extend(body);
    words.extend([
        0xBEFE_040C, // s_mov_b64 exec, s[12:13]
        0x7E08_0206, // v_mov_b32 v4, s6
        0xE070_1000,
        0x8002_0103, // buffer_store_dword v1, v3, s[8:11], 0 offen
        0xBF81_0000, // s_endpgm
    ]);
    NativeProgram {
        stage: HwStage::Cs,
        wave64: true,
        vgprs: 8,
        layout: vec![
            RootParam {
                root: RootType::Constants,
                table_offset_dwords: 0,
                sgprs: 4,
                base_register: 0,
                user_data: 0,
                slot: 0,
            },
            RootParam {
                root: RootType::Table,
                table_offset_dwords: 0,
                sgprs: 1,
                base_register: 0,
                user_data: 4,
                slot: 1,
            },
        ],
        used: vec![UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 4,
            slot: 1,
        }],
        // 5 user SGPRs, one 512-byte LDS granule (128 dwords)
        io: StageIo::Cs(ComputeIo {
            rsrc2: 5 << 1 | 1 << 15,
            threads: [THREADS, 1, 1],
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    }
}

struct Restore(Option<u32>, Option<bool>, Option<bool>);

impl Drop for Restore {
    fn drop(&mut self) {
        TEST_HOST_WAVE.set(self.0);
        crate::emit::TEST_STRICT_LOCAL.set(self.1);
        crate::emit::TEST_GROUP_WIDE.set(self.2);
    }
}

/// A host of 32-lane waves, local waves only where exact, the switch as `on`.
fn host_32(on: bool) -> Restore {
    Restore(
        TEST_HOST_WAVE.replace(Some(32)),
        crate::emit::TEST_STRICT_LOCAL.replace(Some(true)),
        crate::emit::TEST_GROUP_WIDE.replace(Some(on)),
    )
}

fn branch(p: &Program) -> usize {
    (0..p.blocks.len())
        .find(|&b| p.blocks[b].insts.last().is_some_and(|i| i.name().starts_with("s_cbranch")))
        .expect("the branch")
}

/// The program through public emission, and the wave size its PSV0 asks of the host.
fn emitted(native: &NativeProgram) -> (HostShader, u32) {
    let p = crate::ir::build(&native.code, true, native.vgprs).unwrap();
    let st = crate::structure::structure(&p).unwrap();
    let io = crate::io::raise(&p, native).unwrap();
    let res = crate::resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
    let bytes = crate::dxbc::build(&[]);
    let container = crate::Container::parse(&bytes).unwrap();
    let shader = crate::emit::emit(&container, native, &p, &st, &io, &res).unwrap();
    let dxil = crate::Container::parse(&shader.dxil).unwrap();
    let psv = dxil.part(crate::dxbc::FourCC::PSV0).unwrap().data;
    let wave = u32::from_le_bytes(psv[20..24].try_into().unwrap());
    (shader, wave)
}

#[test]
fn lds_read_and_written_in_one_lane_branch_goes_to_32_lanes_only_with_the_switch() {
    let native = native(SCC0, false);
    {
        let _host = host_32(false);
        assert_eq!(emitted(&native).1, 64, "without the switch the program keeps 64 lanes");
    }
    let _host = host_32(true);
    let (shader, wave) = emitted(&native);
    assert_eq!(wave, 32);
    assert!(shader.independent_waves.is_none());
    if let Ok(mut v) = Validator::load(Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
}

#[test]
fn every_thread_takes_the_branch_and_reaches_every_barrier() {
    for skip in [EXECZ, SCC0] {
        every_thread_takes(skip);
    }
}

fn every_thread_takes(skip: u32) {
    let native = native(skip, false);
    let p = crate::ir::build(&native.code, true, native.vgprs).unwrap();
    let d = branch(&p);
    let wide = HashSet::from([d]);
    let st = crate::structure::structure_wide(&p, &wide).unwrap();
    // (the skip is taken; every thread goes the other way, into the body)
    assert_eq!(forced_wide(&p, &st).get(&d).copied(), Some(Ok(false)));
    let io = crate::io::raise(&p, &native).unwrap();
    let res = crate::resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
    let parts = Parts {
        native: &native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: vec![],
        outputs: vec![],
        prims: vec![],
        mesh_stride: 0,
        independent_waves: None,
    };
    let _host = host_32(true);
    let bad = HashMap::new();
    let mut lower = with_forced_wide(true, || {
        let mut lower = Lower::new(&parts, &bad);
        assert!(lower.body().is_ok());
        lower
    });
    assert!(lower.split_lds && lower.exchange.is_some() && !lower.local);
    assert_eq!(lower.s.wave_size, Some(32));
    lower.s.m.optimise(crate::emit::bitcode::opt::ALL);
    execute::check_phis(&lower.s.m);
    // the counter's, the lane read's exchange, and one before each LDS access at least
    assert!(execute::barriers_on_every_path(&lower.s.m) >= 4);
}

#[test]
fn a_scalar_left_behind_by_the_branch_keeps_it_per_console_wave() {
    let native = native(SCC0, true);
    let p = crate::ir::build(&native.code, true, native.vgprs).unwrap();
    let d = branch(&p);
    let st = crate::structure::structure_wide(&p, &HashSet::from([d])).unwrap();
    assert!(matches!(forced_wide(&p, &st).get(&d), Some(Err(w)) if w.contains("scalar")));
    let _host = host_32(true);
    assert_eq!(emitted(&native).1, 64);
}

/// On a GPU of 32-lane waves: each thread below s0 reads what its partner in the other half
/// of its console wave wrote, as the console's lockstep wave does, also where the other
/// console wave of the group skips the branch.
#[test]
fn the_partners_write_is_read_on_the_card_as_in_a_64_lane_wave() {
    let native = native(SCC0, false);
    let _host = host_32(true);
    let (shader, wave) = emitted(&native);
    assert_eq!(wave, 32);
    let _one = crate::emit::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return;
    };
    let (root, pso) = match host.compute_pipeline(&shader) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("the hardware device does not take this program: {e}; not run");
            return;
        }
    };
    for below in [THREADS, 96, 64, 40, 0] {
        let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        for u in &shader.user_data {
            if let (RootType::Constants, 0, 0) = (u.root, u.slot, u.dword) {
                ud[u.index as usize] = below;
            }
        }
        let out = host
            .dispatch(
                &root,
                &pso,
                &Dispatch {
                    roots: vec![(0, Root::Constants(ud))],
                    views: &[View::RawUav(THREADS)],
                    samplers: &[],
                    groups: [1, 1, 1],
                },
            )
            .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
        for t in 0..THREADS {
            // the 64-lane console: lane 32's index outside the branch, the partner's inside
            // (unwritten LDS where the partner is not in the branch: not compared)
            let expected = if t >= below {
                t & !63 | 32
            } else if t ^ 32 < below {
                t ^ 32
            } else {
                continue;
            };
            assert_eq!(out[0][t as usize], expected, "thread {t}, {below} in the branch");
        }
    }
    eprintln!("ran on the hardware device: every partner's write read, in every case");
}

/// Thread t of a group of 128, below s0 only (an early-out for the rest): the UAV descriptor
/// s[8:11] is loaded inside a lane branch over a lane mask in the same registers, with LDS
/// dword t = t; after the join (EXEC restored) LDS dword t ^ 32, which the console's lockstep
/// wave has written by then, plus lane 32's thread index (`v_readlane` at the start) goes to
/// dword t through that descriptor. Only lanes that took the branch store, so it is the loaded
/// one wherever it is used; once the early-out is left out, reaching the store no longer says
/// so, its EXEC still does.
fn joined_descriptor() -> NativeProgram {
    let inner = [
        0xF408_0202,
        0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], 0
        0xD834_0000,
        0x0000_0003, // ds_write_b32 v3, v0
        0xBF8C_0000, // s_waitcnt 0
    ];
    let after = [
        0xBEFE_0414, // s_mov_b64 exec, s[20:21]
        0x3A04_00A0, // v_xor_b32 v2, 32, v0
        0x3404_0482, // v_lshlrev_b32 v2, 2, v2
        0xD8D8_0000,
        0x0100_0002, // ds_read_b32 v1, v2
        0xBF8C_0000, // s_waitcnt 0
        0x4A02_0206, // v_add_nc_u32 v1, s6, v1
        0xE070_1000,
        0x8002_0103, // buffer_store_dword v1, v3, s[8:11], 0 offen
    ];
    let mut words = vec![
        0xBE85_0380, // s_mov_b32 s5, 0
        0x3406_0082, // v_lshlrev_b32 v3, 2, v0
        0xD760_0006,
        0x0001_4100, // v_readlane_b32 s6, v0, 32
        0x7D88_0000, // v_cmp_gt_u32 vcc, s0, v0
        0xBEA8_046A, // s_mov_b64 s[40:41], vcc
        0xBE8C_2428, // s_and_saveexec_b64 s[12:13], s[40:41]
        EXECZ | (4 + inner.len() + after.len()) as u32, // the early-out, to s_endpgm
        0xBE88_047E, // s_mov_b64 s[8:9], exec
        0xBE8A_047E, // s_mov_b64 s[10:11], exec
        0xBE94_2428, // s_and_saveexec_b64 s[20:21], s[40:41]
        EXECZ | inner.len() as u32,
    ];
    words.extend(inner);
    words.extend(after);
    words.push(0xBF81_0000); // s_endpgm
    NativeProgram {
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        ..native(EXECZ, false)
    }
}

#[test]
fn a_descriptor_loaded_in_a_lane_branch_is_traced_by_the_lanes_that_store_through_it() {
    let native = joined_descriptor();
    let p = crate::ir::build(&native.code, true, native.vgprs).unwrap();
    let q = crate::ir::without_early_outs(&p, false).expect("the early-out");
    let st = crate::structure::structure(&q).unwrap();
    let track = || crate::resource::track(&q, &native.layout, native.stage, Some(&st));
    {
        let _host = host_32(false);
        let r = track();
        assert!(
            matches!(&r, Err(ShaderError::Unsupported { what, .. })
                if *what == "a descriptor neither loaded nor built"),
            "{r:?}"
        );
    }
    let _host = host_32(true);
    let res = track().unwrap();
    assert!(res.descriptors.iter().all(|d| matches!(d.source, Source::Loaded { .. })));
}

#[test]
fn a_descriptor_loaded_in_a_lane_branch_goes_to_32_lanes_only_with_the_switch() {
    let native = joined_descriptor();
    {
        let _host = host_32(false);
        assert_eq!(emitted(&native).1, 64, "without the switch the program keeps 64 lanes");
    }
    let _host = host_32(true);
    let (shader, wave) = emitted(&native);
    assert_eq!(wave, 32);
    assert!(shader.independent_waves.is_none());
    if let Ok(mut v) = Validator::load(Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    // the form without the early-out, where every thread reaches the barriers
    let p = crate::ir::build(&native.code, true, native.vgprs).unwrap();
    let q = crate::ir::without_early_outs(&p, false).unwrap();
    let st = crate::structure::structure(&q).unwrap();
    let io = crate::io::raise(&q, &native).unwrap();
    let res = crate::resource::track(&q, &native.layout, native.stage, Some(&st)).unwrap();
    let parts = Parts {
        native: &native,
        p: &q,
        st: &st,
        io: &io,
        res: &res,
        inputs: vec![],
        outputs: vec![],
        prims: vec![],
        mesh_stride: 0,
        independent_waves: None,
    };
    let bad = HashMap::new();
    let mut lower = Lower::new(&parts, &bad);
    assert!(lower.body().is_ok());
    assert!(lower.split_lds && !lower.local);
    assert_eq!(lower.s.wave_size, Some(32));
    lower.s.m.optimise(crate::emit::bitcode::opt::ALL);
    execute::check_phis(&lower.s.m);
    // where the two host waves meet around the LDS accesses
    assert!(execute::barriers_on_every_path(&lower.s.m) >= 2);
}

/// On a GPU of 32-lane waves: each thread below s0 whose partner is below it too reads the
/// partner's write and stores it through the descriptor loaded in the branch.
#[test]
fn the_descriptor_loaded_in_the_branch_stores_on_the_card_as_in_a_64_lane_wave() {
    let native = joined_descriptor();
    let _host = host_32(true);
    let (shader, wave) = emitted(&native);
    assert_eq!(wave, 32);
    let _one = crate::emit::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 hardware device; not run");
        return;
    };
    let (root, pso) = match host.compute_pipeline(&shader) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("the hardware device does not take this program: {e}; not run");
            return;
        }
    };
    for below in [THREADS, 96, 64, 40, 0] {
        let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        for u in &shader.user_data {
            if let (RootType::Constants, 0, 0) = (u.root, u.slot, u.dword) {
                ud[u.index as usize] = below;
            }
        }
        let out = host
            .dispatch(
                &root,
                &pso,
                &Dispatch {
                    roots: vec![(0, Root::Constants(ud))],
                    views: &[View::RawUav(THREADS)],
                    samplers: &[],
                    groups: [1, 1, 1],
                },
            )
            .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
        for t in (0..below).filter(|t| t ^ 32 < below) {
            let expected = (t ^ 32) + (t & !63 | 32);
            assert_eq!(out[0][t as usize], expected, "thread {t}, {below} below");
        }
    }
    eprintln!("ran on the hardware device: every store through the loaded descriptor");
}
