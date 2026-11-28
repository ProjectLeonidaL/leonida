//! Authored cube-gradient regression adapted from DexNom's 921d8d3.
//! https://github.com/DexNom/VIctory/commit/921d8d3b43b44f3a6ca4b44095edd3c4baf10a87
//! Every face stores its mip level in red, exposing the selected LOD directly.

use super::{HostShader, lower};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{io, ir, resource, structure};

const SIZE: usize = 64;
// Opcode, footprint in texels, minimum/explicit LOD, expected sampled level.
const CASES: [(u32, f32, f32, f32); 10] = [
    (0x22, 1.0, 0.0, 0.0),
    (0x22, 2.0, 0.0, 1.0),
    (0x22, 16.0, 0.0, 4.0),
    (0x22, 0.000001, 0.0, 0.0),
    (0x22, 0.0, 0.0, 0.0),
    (0x23, 1.0, 3.0, 3.0),
    (0x23, 16.0, 2.0, 4.0),
    (0x23, 0.0, 2.0, 2.0),
    (0x24, 0.0, 2.0, 2.0),
    (0x27, 0.0, 0.0, 0.0),
];

fn program() -> NativeProgram {
    let mut words = vec![
        0xBE81_0380,
        0xBE83_0380, // s1, s3 = 0 (upper halves of the tables)
        0xF40C_0400,
        0xFA00_0000, // cube SRV to s[16:23] from s[0:1]
        0xF408_0300,
        0xFA00_0020, // raw output UAV to s[12:15] from s[0:1]+32
        0xF408_0601,
        0xFA00_0000, // sampler to s[24:27] from s[2:3]
        0x7E1E_0280, // v15 = 0 for output byte offsets
    ];
    for (i, &(op, footprint, lod, _)) in CASES.iter().enumerate() {
        let delta = footprint / SIZE as f32;
        let mut addr = Vec::new();
        if op == 0x22 || op == 0x23 {
            addr.extend([delta, 0.0, 0.0, delta]); // derivatives along the face
        }
        addr.extend([1.5f32, 1.5, 0.0]); // positive X face, center
        if op == 0x23 || op == 0x24 {
            addr.push(lod);
        }
        for (reg, value) in addr.iter().enumerate() {
            words.extend([0x7E00_02FF | ((reg as u32) << 17), value.to_bits()]);
        }
        // MIMG, CUBE, dmask:R, v0... inputs, SRV s16, sampler s24, result v10.
        words.extend([0xF000_0118 | (op << 18), 0x00C4_0A00]);
        // buffer_store_dword v10, v15, s[12:15], 0 offen offset:i*4
        words.extend([0xE070_1000 | (i as u32 * 4), 0x8003_0A0F]);
    }
    words.push(0xBF81_0000); // s_endpgm
    let table = |slot, user_data| RootParam {
        root: RootType::Table,
        table_offset_dwords: 0,
        sgprs: 1,
        base_register: 0,
        user_data,
        slot,
    };
    let range = |kind, at, dwords, slot, user_data| UsedResource {
        kind,
        root: RootType::Table,
        table_offset_dwords: at,
        lower_bound: 0,
        stride: 0,
        dwords,
        user_data,
        slot,
    };
    NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 16,
        layout: vec![table(0, 0), table(1, 2)],
        used: vec![
            range(ResourceType::Srv, 0, 8, 0, 0),
            range(ResourceType::Uav, 8, 4, 0, 0),
            range(ResourceType::Sampler, 0, 4, 1, 2),
        ],
        io: StageIo::Cs(ComputeIo {
            rsrc2: 3 << 1,
            threads: [1, 1, 1],
        }),
        code: words.into_iter().flat_map(u32::to_le_bytes).collect(),
    }
}

fn shader() -> HostShader {
    let native = program();
    let p = ir::build(&native.code, native.wave64, native.vgprs).unwrap();
    let st = structure::structure(&p).unwrap();
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).unwrap();
    let io = io::raise(&p, &native).unwrap();
    let lowered = lower::lower(&lower::Parts {
        native: &native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
        independent_waves: None,
    })
    .unwrap();
    HostShader {
        stage: HwStage::Cs,
        independent_waves: None,
        dxil: lowered.dxil,
        user_data: lowered.user_data,
        image_bindings: lowered.image_bindings,
        samplers: lowered.samplers,
        static_samplers: Vec::new(),
        root_cbvs: lowered.root_cbvs,
        root_srvs: lowered.root_srvs,
        root_uavs: lowered.root_uavs,
        root_indexed_views: res.root_indexed_views,
    }
}

#[test]
fn cube_gradient_program_lowers_and_validates() {
    let shader = shader();
    crate::Container::parse(&shader.dxil).unwrap();
    #[cfg(windows)]
    if let Ok(mut validator) =
        super::validator::Validator::load(super::validator::Validator::default_path().as_deref())
    {
        validator
            .validate(&mut shader.dxil.clone())
            .expect("DXIL validation");
        eprintln!("cube gradient DXIL validated");
    }
}

/// Set LEONIDA_TEST_CUBE_GRADIENT_LOD to hardware or warp explicitly.
#[cfg(windows)]
#[test]
fn guest_cube_gradient_selects_mip_by_footprint() {
    use super::host::{Dispatch, Host, Root, View};
    use windows::Win32::Graphics::Direct3D12::*;
    let warp = match std::env::var("LEONIDA_TEST_CUBE_GRADIENT_LOD").as_deref() {
        Ok("hardware") => false,
        Ok("warp") => true,
        Err(_) => {
            eprintln!("set LEONIDA_TEST_CUBE_GRADIENT_LOD=hardware or warp for the device test");
            return;
        }
        Ok(_) => panic!("LEONIDA_TEST_CUBE_GRADIENT_LOD must be hardware or warp"),
    };
    let shader = shader();
    const MIPS: u16 = 7;
    let texels: Vec<_> = (0..6)
        .flat_map(|_| {
            (0..MIPS).flat_map(|mip| std::iter::repeat_n([f32::from(mip); 4], (SIZE >> mip).pow(2)))
        })
        .collect();
    let sampler = D3D12_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MaxLOD: f32::MAX,
        MaxAnisotropy: 1,
        ..Default::default()
    };
    let host = Host::new(warp).expect("the explicitly requested SM 6.6 device");
    let (root, pso) = host.compute_pipeline(&shader).unwrap();
    let out = host
        .dispatch_bounded(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(
                    0,
                    Root::Constants(vec![0; shader.user_data_dwords() as usize]),
                )],
                views: &[
                    View::CubeMips(SIZE as u32, MIPS, &texels),
                    View::RawUav(CASES.len() as u32),
                ],
                samplers: &[sampler],
                groups: [1, 1, 1],
            },
            std::time::Duration::from_secs(5),
        )
        .unwrap();
    let levels: Vec<_> = out[0].iter().copied().map(f32::from_bits).collect();
    eprintln!("cube sampled levels: {levels:?}");
    assert_eq!(levels.len(), CASES.len());
    for (i, (actual, expected)) in levels.iter().zip(CASES.map(|c| c.3)).enumerate() {
        assert!(
            (actual - expected).abs() < 0.0001,
            "case {i}: {actual}, expected {expected}"
        );
    }
}
