//! Bind-time numeric typing for a bounded set of ordinary volume accesses.

use super::*;

impl Lower<'_> {
    /// Only a single statically resolved table descriptor can share a uniform class word.
    /// Arrays, waterfalls, fetched descriptors and merged sources remain legacy accesses.
    pub(super) fn image_binding(&self, src: &Source, uav: bool) -> Option<(u8, u32)> {
        let Source::Loaded { load, first } = src else {
            return None;
        };
        let address = self.scalar.get(load)?;
        let root = self.x.native.layout.get(address.param)?;
        if address.dynamic.is_some()
            || root.root != RootType::Table
            || root.slot >= 64
            || !address.offset.is_multiple_of(4)
            || self.x.res.waterfall.loads.contains(load)
        {
            return None;
        }
        let dword = (address.offset / 4).checked_add(u32::from(*first))?;
        if !dword.is_multiple_of(8) || dword.checked_mul(4).is_none() {
            return None;
        }
        let kind = if uav {
            ResourceType::Uav
        } else {
            ResourceType::Srv
        };
        let mut matches = self.x.native.used.iter().filter(|u| {
            (u.root, u.slot, u.user_data, u.table_offset_dwords)
                == (RootType::Table, root.slot, root.user_data, dword)
        });
        let used = matches.next()?;
        // The static SMEM address selects the image descriptor independently of stride metadata.
        (used.kind == kind && used.dwords == 8 && matches.next().is_none())
            .then_some((root.slot, dword))
    }

    fn image_class(&mut self, slot: u8, dword: u32, uav: bool) -> Result<V, Why> {
        let known = self
            .image_bindings
            .iter()
            .find(|b| (b.slot, b.dword) == (slot, dword));
        let word = if let Some(b) = known {
            if b.uav != uav {
                return Err("conflicting image binding categories");
            }
            b.word
        } else {
            let word = self.next_host_word();
            if word >= 64 {
                return Err("image class exceeds the root constant budget");
            }
            self.image_bindings.push(ImageBinding {
                word,
                slot,
                dword,
                uav,
            });
            word
        };
        Ok(self.user_at(word))
    }

    /// The class is uniform, but EXEC can still disable individual stores. Both paths use
    /// the original coordinates and descriptor index; only one memory access executes.
    pub(super) fn classified_image(
        &mut self,
        inst: &Inst,
        site: Site,
        src: &Source,
        binding: (u8, u32),
        cs: [V; 3],
        guard: Option<V>,
    ) -> Result<(), Why> {
        let store = inst.name() == "image_store";
        let uav = store
            || self
                .used_kind(src)
                .is_some_and(|u| u.kind == ResourceType::Uav);
        let class = self.image_class(binding.0, binding.1, uav)?;
        let uint = self.s.m.i32(super::super::IMAGE_CLASS_R32_UINT);
        let typed = self.s.m.cmp(self.cur, pred::EQ, class, uint);
        let kind = 4 | if uav { 0x1000 } else { 0 };
        let fh = self.handle(site, src, false, (kind, 0x409))?;
        let ih = self.handle(site, src, false, (kind, 0x105))?;
        let Fields::Image { data, .. } = &inst.fields else {
            unreachable!()
        };
        let Operand::Vector { reg, .. } = *data else {
            return Err("image data register");
        };
        let i32t = self.s.t.i32;
        let iu = self.s.m.undef(i32t);
        let (raw, floats) = if store {
            let value = inst.read(Reg::V(reg)).ok_or("image store data")?;
            let raw = self.i32_of(value)?;
            let mut floats = Vec::new();
            for k in 0..4 {
                let v = inst.read(Reg::V(reg + k)).ok_or("image store data")?;
                floats.push(self.float_of(v)?);
            }
            (raw, floats)
        } else {
            (iu, Vec::new())
        };
        let guarded = if store {
            self.begin_guarded(guard)
        } else {
            None
        };
        let (int_block, float_block, join) = (
            self.s.m.new_block(),
            self.s.m.new_block(),
            self.s.m.new_block(),
        );
        self.s
            .m
            .terminate(self.cur, Term::CondBr(typed, int_block, float_block));
        if store {
            let mut args = vec![ih];
            args.extend(cs);
            // DXIL requires all four operands of a typed UAV store, even for scalar views.
            args.extend([raw; 4]);
            args.push(self.s.m.i8(0xf));
            self.s.op(
                int_block,
                "textureStore.i32",
                Attr::NoUnwind,
                self.s.t.void,
                67,
                &args,
            );
            let mut args = vec![fh];
            args.extend(cs);
            args.extend(floats);
            args.push(self.s.m.i8(15));
            self.s.op(
                float_block,
                "textureStore.f32",
                Attr::NoUnwind,
                self.s.t.void,
                67,
                &args,
            );
            self.s.m.terminate(int_block, Term::Br(join));
            self.s.m.terminate(float_block, Term::Br(join));
            self.cur = join;
            self.end_guarded(guarded, &[]);
        } else {
            let mip = if uav {
                // The legacy branch still declares a four-component typed UAV load.
                self.s.flags |= flags::TYPED_UAV_LOADS;
                iu
            } else {
                self.s.m.i32(0)
            };
            let mut args = vec![ih, mip];
            args.extend(cs);
            args.extend([iu; 3]);
            let ret = self.s.m.named_struct("dx.types.ResRet.i32", vec![i32t; 5]);
            let ir = self
                .s
                .op(int_block, "textureLoad.i32", Attr::ReadOnly, ret, 66, &args);
            let ix = self.s.m.extract(int_block, ir, 0);
            args[0] = fh;
            let fr = self.s.op(
                float_block,
                "textureLoad.f32",
                Attr::ReadOnly,
                self.s.t.res_ret,
                66,
                &args,
            );
            let fx = self.s.m.extract(float_block, fr, 0);
            let fx = self.s.m.cast(float_block, Cast::Bitcast, fx, i32t);
            self.s.m.terminate(int_block, Term::Br(join));
            self.s.m.terminate(float_block, Term::Br(join));
            let result = self.s.m.phi(join, i32t);
            self.s
                .m
                .set_phi(result, vec![(ix, int_block), (fx, float_block)]);
            self.cur = join;
            self.write(inst, Reg::V(reg), result, guard);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{ComputeIo, NativeProgram, RootParam, StageIo, UsedResource};
    use crate::{io, ir, resource, structure};

    fn program() -> NativeProgram {
        let words: [u32; 20] = [
            0xBE81_0380, // s1 = 0
            0xF40C_0400,
            0xFA00_0020, // 3D UAV at table dword 8 -> s[16:23]
            0xF408_0300,
            0xFA00_0040, // raw output at dword 16 -> s[12:15]
            0xF000_0110,
            0x0004_0A00, // image_load X, coordinates v[0:2]
            0x7E28_02FF,
            0x8000_0000, // v20 = literal high bit (float -0)
            0x7E2A_0314,
            0x7E2C_0314,
            0x7E2E_0314, // v21..23 = v20
            0xF020_0F10,
            0x0004_1400, // image_store RGBA to same UAV
            0xBF8C_3F70, // wait before consuming the loaded value
            0x3A14_14F2, // XOR with normal float bits before the raw sink
            0x7E06_0280, // raw byte offset v3 = 0
            0xE070_1000,
            0x8003_0A03, // raw store v10
            0xBF81_0000,
        ];
        NativeProgram {
            stage: HwStage::Cs,
            wave64: false,
            vgprs: 24,
            layout: vec![RootParam {
                root: RootType::Table,
                table_offset_dwords: 0,
                sgprs: 1,
                base_register: 0,
                user_data: 0,
                slot: 2,
            }],
            used: [(8, 8), (16, 4)]
                .into_iter()
                .map(|(dword, dwords)| UsedResource {
                    kind: ResourceType::Uav,
                    root: RootType::Table,
                    table_offset_dwords: dword,
                    lower_bound: 0,
                    stride: 0,
                    dwords,
                    user_data: 0,
                    slot: 2,
                })
                .collect(),
            io: StageIo::Cs(ComputeIo {
                rsrc2: (2 << 11) | (1 << 1),
                threads: [4, 2, 1],
            }),
            code: words.into_iter().flat_map(u32::to_le_bytes).collect(),
        }
    }

    fn with_parts<T>(native: &NativeProgram, f: impl FnOnce(&Parts<'_>) -> T) -> T {
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs).unwrap();
        let st = structure::structure(&p).unwrap();
        let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
        let io = io::raise(&p, native).unwrap();
        f(&Parts {
            native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs: vec![],
            prims: vec![],
            mesh_stride: 0,
            independent_waves: None,
        })
    }

    #[test]
    fn real_emission_deduplicates_classes_and_guards_each_numeric_access() {
        with_parts(&program(), |parts| {
            let bad = HashMap::new();
            let mut lower = Lower::new(parts, &bad);
            lower.uint_images = true;
            lower.body().unwrap_or_else(|_| panic!("authored lowering"));
            assert_eq!(
                lower.image_bindings,
                [ImageBinding {
                    word: 1,
                    slot: 2,
                    dword: 8,
                    uav: true
                }]
            );
            super::super::super::bitcode::execute::check_image_classes(&lower.s.m, 1, 1);
            lower.s.m.optimise(super::super::super::bitcode::opt::ALL);
            super::super::super::bitcode::execute::check_image_classes(&lower.s.m, 1, 1);
            let bytes = lower.s.finish();
            crate::Container::parse(&bytes).unwrap();
            #[cfg(windows)]
            if let Ok(mut validator) = super::super::super::validator::Validator::load(
                super::super::super::validator::Validator::default_path().as_deref(),
            ) {
                validator
                    .validate(&mut bytes.clone())
                    .expect("DXIL validator");
            }
        });
    }

    #[cfg(windows)]
    #[test]
    fn sdk_rejects_incomplete_typed_store_mask() {
        let Some(path) = std::env::var_os("LEONIDA_DXIL_VALIDATOR") else {
            eprintln!("skipped: set LEONIDA_DXIL_VALIDATOR to the SDK dxil.dll");
            return;
        };
        let path = std::path::Path::new(&path);
        let mut validator = super::super::super::validator::Validator::load(Some(path))
            .expect("load configured SDK validator");
        let mut broken = with_parts(&program(), |parts| {
            let bad = HashMap::new();
            let mut lower = Lower::new(parts, &bad);
            lower.uint_images = true;
            lower.body().unwrap_or_else(|_| panic!("authored lowering"));
            super::super::super::bitcode::execute::force_incomplete_typed_store_mask(&mut lower.s.m);
            lower.s.finish()
        });
        let error = validator.validate(&mut broken).expect_err("DXIL must reject the mask");
        assert!(
            error.contains("store on typed uav must write to all four components of the UAV"),
            "unexpected validator error: {error}"
        );
        eprintln!("SDK rejected the authored incomplete typed store mask");
    }

    #[test]
    fn both_store_branches_remain_inside_the_original_exec_guard() {
        let mut native = program();
        native.wave64 = true;
        let words: Vec<_> = native
            .code
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let mut guarded = words[..12].to_vec();
        guarded.extend([0x7D84_0000, 0xBE84_246A]); // compare, s_and_saveexec s[4:5]
        guarded.extend_from_slice(&words[12..14]);
        guarded.push(0xBEFE_0404); // restore EXEC
        guarded.extend_from_slice(&words[14..]);
        native.code = guarded.into_iter().flat_map(u32::to_le_bytes).collect();
        with_parts(&native, |parts| {
            let bad = HashMap::new();
            let mut lower = Lower::new(parts, &bad);
            lower.uint_images = true;
            lower.body().unwrap_or_else(|e| match e {
                Stop::Fail(e) => panic!("EXEC program: {e}"),
                Stop::Retry(_, why) => panic!("EXEC retry: {why}"),
            });
            super::super::super::bitcode::execute::check_image_classes(&lower.s.m, 1, 1);
            super::super::super::bitcode::execute::check_image_store_exec_guard(&lower.s.m);
            lower.s.m.optimise(super::super::super::bitcode::opt::ALL);
            super::super::super::bitcode::execute::check_image_classes(&lower.s.m, 1, 1);
            super::super::super::bitcode::execute::check_image_store_exec_guard(&lower.s.m);
        });
    }

    #[test]
    fn disabled_lowering_is_byte_identical_to_the_unconfigured_lower() {
        with_parts(&program(), |parts| {
            let bad = HashMap::new();
            let original = Lower::new(parts, &bad)
                .run()
                .unwrap_or_else(|_| panic!("legacy"));
            let off = lower_with_image_classes(parts, false).unwrap();
            assert_eq!(off.dxil, original.dxil);
            assert_eq!(off.user_data, original.user_data);
            assert!(off.image_bindings.is_empty());
            let on = lower_with_image_classes(parts, true).unwrap();
            assert_ne!(on.dxil, off.dxil);
        });
    }

    #[test]
    fn static_image_selection_does_not_use_declared_stride() {
        let mut native = program();
        let emit = |native: &NativeProgram| {
            with_parts(native, |parts| lower_with_image_classes(parts, true).unwrap())
        };
        let expected = emit(&native);
        assert_eq!(expected.image_bindings.len(), 1);
        for stride in [4, 8] {
            native.used[0].stride = stride;
            let actual = emit(&native);
            assert_eq!(
                actual.image_bindings, expected.image_bindings,
                "stride {stride}"
            );
            assert_eq!(actual.dxil, expected.dxil, "stride {stride}");
        }
    }

    #[test]
    fn static_offset_matches_actual_smem_base_immediate_and_soffset_with_wrap() {
        let mut native = program();
        let mut code = vec![
            0xBE81_0380,
            0x8004_A000, // s4 = table + 32
            0xBE85_0380,
            0xBE82_0390, // s5 = 0, s2 = 16
            0xF40C_0402,
            0x0400_0010, // descriptor at s[4:5] + 16 + s2
            0xF408_0300,
            0xFA00_0060, // raw output at table + 96
        ];
        code.extend(
            native.code[20..]
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap())),
        );
        native.code = code.into_iter().flat_map(u32::to_le_bytes).collect();
        native.used[0].table_offset_dwords = 16;
        native.used[1].table_offset_dwords = 24;
        with_parts(&native, |parts| {
            let bad = HashMap::new();
            let mut lower = Lower::new(parts, &bad);
            lower.uint_images = true;
            lower
                .body()
                .unwrap_or_else(|_| panic!("static-offset program"));
            assert_eq!(lower.image_bindings[0].dword, 16);
            let src = lower
                .descriptors
                .values()
                .copied()
                .find(|s| lower.image_binding(s, true).is_some())
                .unwrap();
            let Source::Loaded { load, first } = *src else {
                unreachable!()
            };
            let index = lower.heap_index(load, first, false).unwrap();
            for table in [0u32, 128, 0xffff_ffe0, 0xffff_fff0] {
                assert_eq!(
                    super::super::super::bitcode::execute::table_index(&lower.s.m, index, table),
                    table.wrapping_add(64) >> 5
                );
            }
        });
    }

    #[test]
    fn unsupported_width_dimensions_and_masks_keep_the_existing_lowering() {
        for variant in 0..5 {
            let mut native = program();
            let mut words: Vec<_> = native
                .code
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            match variant {
                0 => {
                    words[6] |= 1 << 31;
                    words[13] |= 1 << 31;
                } // d16
                1 => {
                    words[6] |= 1 << 30;
                    words[13] |= 1 << 30;
                } // a16
                2 => {
                    words[5] ^= 0x18;
                    words[12] ^= 0x18;
                } // 2D
                3 => {
                    words[5] |= 0x200;
                    words[12] &= !0x800;
                } // other masks
                _ => {
                    words[5] |= 1 << 18;
                    words[12] |= 1 << 18;
                } // explicit mip
            }
            native.code = words.into_iter().flat_map(u32::to_le_bytes).collect();
            with_parts(&native, |parts| {
                let off = lower_with_image_classes(parts, false);
                let on = lower_with_image_classes(parts, true);
                match (off, on) {
                    (Ok(off), Ok(on)) => {
                        assert!(on.image_bindings.is_empty());
                        assert_eq!(on.dxil, off.dxil);
                    }
                    (Err(off), Err(on)) => assert_eq!(on, off),
                    _ => panic!("unsupported scope must not change admission"),
                }
            });
        }
    }

    #[test]
    fn class_words_share_the_host_allocator_and_refuse_the_limit() {
        with_parts(&program(), |parts| {
            let bad = HashMap::new();
            let mut lower = Lower::new(parts, &bad);
            lower.host_word(RootType::Other(250), 0, 0);
            lower.image_class(2, 8, true).unwrap();
            lower.host_word(RootType::Other(250), 0, 1);
            assert_eq!(lower.image_bindings[0].word, 2);
            assert!(lower.user_reads.contains_key(&1));
            assert!(lower.user_reads.contains_key(&3));
            for dword in 2..62 {
                lower.host_word(RootType::Other(250), 0, dword);
            }
            assert!(lower.image_class(2, 16, true).is_err());
        });
    }

    #[test]
    fn static_binding_proof_rejects_merged_dynamic_and_inconsistent_sources() {
        with_parts(&program(), |parts| {
            let bad = HashMap::new();
            let mut lower = Lower::new(parts, &bad);
            let src = lower
                .descriptors
                .values()
                .copied()
                .find(|s| lower.image_binding(s, true).is_some())
                .unwrap()
                .clone();
            assert_eq!(lower.image_binding(&src, true), Some((2, 8)));
            assert_eq!(lower.image_binding(&src, false), None);
            let mut res = parts.res.clone();
            let Source::Loaded {
                load: selected_load,
                ..
            } = src
            else {
                unreachable!()
            };
            res.waterfall.loads.push(selected_load);
            let changed = Parts {
                res: &res,
                inputs: vec![],
                outputs: vec![],
                prims: vec![],
                native: parts.native,
                p: parts.p,
                st: parts.st,
                io: parts.io,
                mesh_stride: 0,
                independent_waves: None,
            };
            let waterfall = Lower::new(&changed, &bad);
            assert!(waterfall.image_binding(&src, true).is_none());
            let Source::Loaded { load, first } = src else {
                unreachable!()
            };
            assert!(
                lower
                    .image_binding(&Source::Fetched { load, first }, true)
                    .is_none()
            );
            assert!(
                lower
                    .image_binding(
                        &Source::Phi {
                            block: 0,
                            args: vec![src.clone()]
                        },
                        true
                    )
                    .is_none()
            );
            lower.scalar.get_mut(&load).unwrap().dynamic = Some(Value(0));
            assert!(lower.image_binding(&src, true).is_none());
            lower.scalar.get_mut(&load).unwrap().dynamic = None;
            lower.scalar.get_mut(&load).unwrap().offset += 4;
            assert!(lower.image_binding(&src, true).is_none());
        });
        for variant in 0..4 {
            let mut native = program();
            match variant {
                0 => native.used[0].kind = ResourceType::Cbv,
                1 => native.used[0].user_data = 7,
                2 => native.used.push(native.used[0]),
                _ => native.used[0].dwords = 16,
            }
            with_parts(&native, |parts| {
                let bad = HashMap::new();
                let lower = Lower::new(parts, &bad);
                assert!(
                    lower
                        .descriptors
                        .values()
                        .all(|s| lower.image_binding(s, true).is_none())
                );
            });
        }
    }
}
