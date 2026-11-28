//! Original programs for checking exact integer volume loads and stores.

use leonida_shader::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use leonida_shader::{Container, emit, io, ir, resource, structure};

const WIDTH: u32 = 4;
const HEIGHT: u32 = 2;
const DEPTH: u32 = 13;
const WORDS: usize = (WIDTH * HEIGHT * DEPTH) as usize;
const SPECIAL: [u32; 6] = [
    0xffff_ffff,
    0x7fff_ffff,
    0x7fc0_0000,
    0x7f80_0000,
    0xff80_0000,
    0x8000_0000,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Store {
    Computed,
    Literal(u32),
    Runtime,
}

#[cfg(windows)]
mod hardware;

fn native_program(input_kind: ResourceType) -> NativeProgram {
    native_program_for(input_kind, Store::Computed)
}

fn native_program_for(input_kind: ResourceType, store: Store) -> NativeProgram {
    let words: [u32; 37] = [
        0xBE81_0380, // s_mov_b32 s1, 0
        0xF40C_0400,
        0xFA00_0000, // s_load_dwordx8 s[16:23], s[0:1], 0 (input volume)
        0xF40C_0600,
        0xFA00_0020, // s_load_dwordx8 s[24:31], s[0:1], 32 (output volume)
        0xF408_0300,
        0xFA00_0040, // s_load_dwordx4 s[12:15], s[0:1], 64 (raw output)
        0xBF8C_C07F, // s_waitcnt lgkmcnt(0)
        0xF000_0110,
        0x0004_0A00, // image_load v10, v[0:2], s[16:23], dmask R, 3D
        0x3428_009F, // v_lshlrev_b32 v20, 31, v0 (high bit of x)
        0x342A_0485, // v_lshlrev_b32 v21, 5, v2
        0x342C_0283, // v_lshlrev_b32 v22, 3, v1
        0x382A_2B16, // v_or_b32 v21, v22, v21
        0x382A_2A81, // v_or_b32 v21, 1, v21 (nonzero low bits)
        0x382C_2B14, // v_or_b32 v22, v20, v21 (mixed word)
        0x2C2E_0081, // v_lshrrev_b32 v23, 1, v0 (retain low bits only for x >= 2)
        0xD569_0017,
        0x0002_2F15, // v_mul_lo_u32 v23, v21, v23
        0x382E_2EFF,
        0x8000_0000, // v_or_b32 v23, 0x80000000, v23
        0x3630_2F16, // v_and_b32 v24, v22, v23
        0x7E32_0318, // v_mov_b32 v25, v24
        0x7E34_0318, // v_mov_b32 v26, v24
        0x7E36_0318, // v_mov_b32 v27, v24
        0xF020_0F10,
        0x0006_1800, // image_store v[24:27], v[0:2], s[24:31], dmask RGBA, 3D
        0x3406_0485, // v_lshlrev_b32 v3, 5, v2
        0x3408_0284, // v_lshlrev_b32 v4, 4, v1
        0x340A_0082, // v_lshlrev_b32 v5, 2, v0
        0x3806_0903, // v_or_b32 v3, v3, v4
        0x3806_0B03, // v_or_b32 v3, v3, v5 (byte offset into raw output)
        0xBF8C_3F70, // s_waitcnt vmcnt(0)
        0x3A14_14F2, // v_xor_b32 v10, 0x3f800000, v10 (normal bits in the raw store)
        0xE070_1000,
        0x8003_0A03, // buffer_store_dword v10, v3, s[12:15], 0 offen
        0xBF81_0000, // s_endpgm
    ];
    let code = if store == Store::Computed {
        words.to_vec()
    } else {
        let mut code = words[..10].to_vec();
        // Compute each thread's unique byte address before the independent store source.
        code.extend_from_slice(&words[27..32]);
        match store {
            Store::Literal(value) => {
                code.extend_from_slice(&[0x7E30_02FF, value]); // v_mov_b32 v24, literal
            }
            Store::Runtime => {
                code.extend_from_slice(&[
                    0xE030_1000,
                    0x8003_1803, // buffer_load_dword v24, v3, s[12:15], 0 offen
                    0xBF8C_3F70, // s_waitcnt vmcnt(0) before using the runtime word
                ]);
            }
            Store::Computed => unreachable!(),
        }
        code.extend_from_slice(&words[22..27]); // replicate v24 and store the volume
        code.extend_from_slice(&words[32..]); // independent image-load result into raw output
        code
    };
    NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 28,
        layout: vec![RootParam {
            root: RootType::Table,
            table_offset_dwords: 0,
            sgprs: 1,
            base_register: 0,
            user_data: 0,
            slot: 0,
        }],
        used: [
            (input_kind, 0, 8),
            (ResourceType::Uav, 8, 8),
            (ResourceType::Uav, 16, 4),
        ]
        .into_iter()
        .map(|(kind, table_offset_dwords, dwords)| UsedResource {
            kind,
            root: RootType::Table,
            table_offset_dwords,
            lower_bound: 0,
            // Image selection comes from the fixed descriptor address, not this
            // buffer-oriented metadata field. Exercise a nonzero value too.
            stride: if dwords == 8 { 4 } else { 0 },
            dwords,
            user_data: 0,
            slot: 0,
        })
        .collect(),
        io: StageIo::Cs(ComputeIo {
            rsrc2: (2 << 11) | (1 << 1),
            threads: [WIDTH, HEIGHT, DEPTH],
        }),
        code: code.iter().flat_map(|word| word.to_le_bytes()).collect(),
    }
}

fn shader(input_kind: ResourceType) -> emit::HostShader {
    shader_for(input_kind, Store::Computed)
}

fn shader_for(input_kind: ResourceType, store: Store) -> emit::HostShader {
    let native = native_program_for(input_kind, store);
    let program = ir::build(&native.code, native.wave64, native.vgprs).expect("authored program");
    let cfg = structure::structure(&program).expect("straight-line structure");
    let resources = resource::track(&program, &native.layout, native.stage, Some(&cfg))
        .expect("authored descriptor table");
    let io = io::raise(&program, &native).expect("authored volume coordinates");
    let mut header = [0u8; 32];
    header[..4].copy_from_slice(b"DXBC");
    header[20..22].copy_from_slice(&1u16.to_le_bytes());
    header[24..28].copy_from_slice(&32u32.to_le_bytes());
    let container = Container::parse(&header).expect("authored empty container");
    let shader = emit::emit(&container, &native, &program, &cfg, &io, &resources)
        .expect("public image-load and image-store emission");
    assert!(shader.independent_waves.is_none());
    assert!(shader.root_cbvs.is_empty());
    assert!(shader.root_srvs.is_empty());
    assert!(shader.root_uavs.is_empty());
    assert!(shader.samplers.is_empty());
    assert!(!shader.user_data.is_empty());
    assert!(
        shader
            .user_data
            .iter()
            .all(|value| { value.root == RootType::Table && value.slot == 0 && value.dword == 0 })
    );
    shader
}

#[cfg(windows)]
fn validated_volume_shaders() -> Vec<(Store, [emit::HostShader; 2])> {
    use leonida_shader::emit::validator::Validator;

    assert_eq!(std::env::var("LEONIDA_UINT_IMAGES").as_deref(), Ok("1"));
    let path = std::env::var_os("LEONIDA_DXIL_VALIDATOR")
        .map(std::path::PathBuf::from)
        .or_else(Validator::default_path);
    let mut validator = Validator::load(path.as_deref())
        .expect("SDK validator required: set LEONIDA_DXIL_VALIDATOR to dxil.dll");
    eprintln!(
        "uint-image SDK validator: {}",
        path.as_deref().unwrap_or_else(|| std::path::Path::new("dxil.dll")).display()
    );

    let programs: Vec<_> = [Store::Computed, Store::Runtime]
        .into_iter()
        .chain(SPECIAL.into_iter().map(Store::Literal))
        .map(|store| {
            let shaders = [ResourceType::Srv, ResourceType::Uav].map(|kind| {
                let mut shader = shader_for(kind, store);
                assert_eq!(shader.image_bindings.len(), 2);
                validator.validate(&mut shader.dxil).unwrap_or_else(|error| {
                    panic!("SDK rejected authored {store:?} {kind:?} shader: {error}")
                });
                shader
            });
            (store, shaders)
        })
        .collect();
    eprintln!("uint-image SDK validated {} authored shaders", programs.len() * 2);
    programs
}

#[cfg(windows)]
#[test]
fn sdk_validates_all_integer_volume_variants() {
    if std::env::var_os("LEONIDA_DXIL_VALIDATOR").is_none() {
        eprintln!("skipped: set LEONIDA_DXIL_VALIDATOR to the SDK dxil.dll");
        return;
    }
    assert_eq!(validated_volume_shaders().len(), 8);
}

#[test]
fn authored_3d_special_words_keep_literal_and_runtime_stores_separate() {
    for store in std::iter::once(Store::Runtime).chain(SPECIAL.into_iter().map(Store::Literal)) {
        for kind in [ResourceType::Srv, ResourceType::Uav] {
            let native = native_program_for(kind, store);
            let decoded = leonida_shader::gfx10::decode(&native.code).unwrap();
            assert_eq!(
                decoded
                    .iter()
                    .filter(|i| i.name() == Some("image_load"))
                    .count(),
                1
            );
            assert_eq!(
                decoded
                    .iter()
                    .filter(|i| i.name() == Some("image_store"))
                    .count(),
                1
            );
            assert_eq!(
                decoded
                    .iter()
                    .filter(|i| i.name() == Some("buffer_load_dword"))
                    .count(),
                usize::from(store == Store::Runtime)
            );
            let shader = shader_for(kind, store);
            assert!(shader.dxil.starts_with(b"DXBC"));
        }
    }
}

#[test]
fn authored_3d_mask_uses_the_public_emitter() {
    use leonida_shader::gfx10::Fields;
    let native = native_program(ResourceType::Srv);
    let instructions = leonida_shader::gfx10::decode(&native.code).unwrap();
    for (name, expected_mask) in [("image_load", 1), ("image_store", 15)] {
        let inst = instructions
            .iter()
            .find(|inst| inst.name() == Some(name))
            .unwrap();
        assert!(
            matches!(inst.fields(false), Fields::Image { dim: 2, dmask, .. }
            if dmask == expected_mask)
        );
    }
    assert_eq!(WORDS, 104);
    for kind in [ResourceType::Srv, ResourceType::Uav] {
        assert!(shader(kind).dxil.starts_with(b"DXBC"));
    }
}
