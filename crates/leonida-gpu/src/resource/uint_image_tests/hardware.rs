//! Separate uploaded image loads and literal/runtime stores on a real D3D12 device.

use super::{DEPTH, HEIGHT, SPECIAL, Store, WIDTH, WORDS, validated_volume_shaders};
use crate::descriptor::{View, host::HostHeap};
use crate::host::transition;
use leonida_shader::emit::HostShader;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::*, Dxgi::*};
use windows::core::Interface;

const RAW_BYTES: u64 = WORDS as u64 * 4;
const SENTINEL: u8 = 0xcd;
const LOAD_TAG: u32 = 0x3f80_0000;
const CASES: usize = 4;

struct Texture(crate::object::This);

impl Drop for Texture {
    fn drop(&mut self) {
        crate::object::slot::<extern "system" fn(crate::object::This) -> u32>(self.0, 0x10)(self.0);
    }
}

fn texture() -> (Texture, ID3D12Resource) {
    use crate::resource::{HeapProperties, RESOURCE_IIDS, ResourceDesc, create_committed};
    let desc = ResourceDesc {
        dimension: 4,
        alignment: 0,
        width: u64::from(WIDTH),
        height: HEIGHT,
        depth_or_array_size: DEPTH as u16,
        mip_levels: 1,
        format: 42,
        sample_count: 1,
        sample_quality: 0,
        layout: 0,
        flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 as u32,
    };
    let properties = HeapProperties {
        kind: 1,
        cpu_pages: 0,
        pool: 0,
        creation_nodes: 0,
        visible_nodes: 0,
    };
    let mut resource = std::ptr::null();
    assert_eq!(
        create_committed(
            std::ptr::null(),
            &properties,
            0,
            &desc,
            D3D12_RESOURCE_STATE_COPY_DEST.0 as u32,
            std::ptr::null(),
            &RESOURCE_IIDS[0],
            &mut resource,
        ),
        crate::object::S_OK
    );
    let owner = Texture(resource);
    let native = crate::resource::host_resource(resource)
        .expect("native volume")
        .clone();
    (owner, native)
}

fn subresource(resource: &ID3D12Resource) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: the returned location borrows a resource retained through execution.
        pResource: unsafe { std::mem::transmute_copy(resource) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    }
}

fn placed(
    buffer: &ID3D12Resource,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    offset: u64,
) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: the returned location borrows a resource retained through execution.
        pResource: unsafe { std::mem::transmute_copy(buffer) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: offset,
                ..footprint
            },
        },
    }
}

fn coordinates(i: usize) -> (u32, u32, u32) {
    let i = i as u32;
    (i % WIDTH, (i / WIDTH) % HEIGHT, i / (WIDTH * HEIGHT))
}

fn seeded_words() -> [u32; WORDS] {
    std::array::from_fn(|i| {
        let (x, y, z) = coordinates(i);
        let payload = (y << 12) | (z << 4) | 1;
        match (x + y + z) % WIDTH {
            0 => 0,
            1 => 0x8000_0000,
            2 => payload,
            _ => 0x8000_0000 | payload,
        }
    })
}

fn expected_stores() -> [u32; WORDS] {
    std::array::from_fn(|i| {
        let (x, y, z) = coordinates(i);
        let payload = z * 32 + y * 8 + 1;
        [0, 0x8000_0000, payload, 0x8000_0000 + payload][x as usize]
    })
}

fn byte_offset(index: usize, row_pitch: usize) -> usize {
    let (x, y, z) = coordinates(index);
    (z * HEIGHT + y) as usize * row_pitch + x as usize * 4
}

fn wait_completed(fence: &ID3D12Fence) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // SAFETY: retained fence for this one submission.
        let value = unsafe { fence.GetCompletedValue() };
        if value == u64::MAX || Instant::now() >= deadline {
            eprintln!("R32_UINT volume fence did not prove retirement: {value}");
            // Keep owners alive while it is unknown whether the commands retired.
            std::process::exit(3);
        }
        if value >= 1 {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Enable explicitly: this requires a hardware D3D12 device and the opt-in translator.
#[test]
fn integer_volumes_preserve_literal_and_runtime_bits() {
    if std::env::var("LEONIDA_TEST_UINT_IMAGES").as_deref() != Ok("hardware") {
        eprintln!("skipped: set LEONIDA_TEST_UINT_IMAGES=hardware");
        return;
    }
    let programs = validated_volume_shaders();
    for (store, shaders) in programs {
        run(store, shaders);
    }
    eprintln!("uint-image suite complete: 32 cases, 104 load and store words per case");
}

fn run(store: Store, [srv, uav]: [HostShader; 2]) {
    assert_ne!(std::env::var("LEONIDA_D3D12_WARP").as_deref(), Ok("1"));
    let shaders = [&srv, &srv, &uav, &uav];
    let cases = [
        ("srv-f32", false),
        ("srv-uint", false),
        ("uav-f32", true),
        ("uav-uint", true),
    ];
    let host = crate::host::host().expect("explicit hardware test needs a device");
    // SAFETY: query the adapter of the retained device used below.
    let adapter = unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
        let adapter: IDXGIAdapter1 = factory
            .EnumAdapterByLuid(host.device.GetAdapterLuid())
            .unwrap();
        adapter.GetDesc1().unwrap()
    };
    assert_eq!(
        adapter.Flags & 2,
        0,
        "hardware fixture cannot use a software adapter"
    );
    assert!(!(adapter.VendorId == 0x1414 && adapter.DeviceId == 0x8c));
    let adapter_name = String::from_utf16_lossy(&adapter.Description);
    eprintln!(
        "actual hardware: {} vendor={:#x} device={:#x}",
        adapter_name.trim_end_matches('\0'),
        adapter.VendorId,
        adapter.DeviceId
    );

    let checker = leonida_shader::emit::host::Host {
        device: host.device.clone(),
    };
    let programs: Vec<_> = shaders
        .iter()
        .map(|shader| {
            checker
                .compute_pipeline(shader)
                .expect("public-emitted volume pipeline")
        })
        .collect();
    let (input_owner, input) = texture();
    let (output_owner, output) = texture();
    let raw = host.uav_buffer(RAW_BYTES).expect("raw load oracle");
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut rows = 0;
    let mut row_bytes = 0;
    // SAFETY: both volumes share this one native subresource shape and format.
    unsafe {
        let desc = output.GetDesc();
        assert_eq!(desc.Dimension, D3D12_RESOURCE_DIMENSION_TEXTURE3D);
        assert_eq!(desc.Format, DXGI_FORMAT_R32_UINT);
        host.device.GetCopyableFootprints(
            &desc,
            0,
            1,
            0,
            Some(&mut footprint),
            Some(&mut rows),
            Some(&mut row_bytes),
            None,
        );
    }
    let f = footprint.Footprint;
    assert_eq!(
        (f.Width, f.Height, f.Depth, f.Format),
        (WIDTH, HEIGHT, DEPTH, DXGI_FORMAT_R32_UINT)
    );
    assert_eq!(
        (footprint.Offset, rows, row_bytes),
        (0, HEIGHT, u64::from(WIDTH * 4))
    );
    let row_pitch = f.RowPitch as usize;
    let volume_bytes = u64::from(f.RowPitch * rows * DEPTH);
    assert_eq!(volume_bytes % 512, 0);
    let special_words = std::array::from_fn(|i| SPECIAL[i % SPECIAL.len()]);
    let expected_load = if store == Store::Computed {
        seeded_words()
    } else {
        special_words
    };
    let expected_store = match store {
        Store::Computed => expected_stores(),
        Store::Literal(value) => [value; WORDS],
        Store::Runtime => special_words,
    };
    let readback_raw = volume_bytes * CASES as u64;
    let readback_size = readback_raw + CASES as u64 * RAW_BYTES;
    let readback = host.read_back_buffer(readback_size).unwrap();
    let upload_size = 2 * volume_bytes + RAW_BYTES;
    let (upload, mapped) = host.upload_buffer(upload_size).unwrap();
    // SAFETY: the upload mapping contains the two fully initialized volumes and raw seed.
    unsafe {
        mapped.write_bytes(SENTINEL, upload_size as usize);
        for (i, word) in expected_load.iter().enumerate() {
            mapped
                .add(byte_offset(i, row_pitch))
                .copy_from_nonoverlapping(word.to_le_bytes().as_ptr(), 4);
        }
        if store == Store::Runtime {
            // Independent integer buffer input: no texture load feeds the texture store.
            // Each thread reads its unique slot before replacing it with the load oracle.
            for (i, word) in special_words.iter().enumerate() {
                mapped
                    .add(2 * volume_bytes as usize + i * 4)
                    .copy_from_nonoverlapping(word.to_le_bytes().as_ptr(), 4);
            }
        }
        upload.Unmap(0, None);
    }

    let descriptors = HostHeap::new(false, 4).expect("native descriptor heap");
    let first = descriptors
        .dynamic(6)
        .expect("six owned dynamic descriptors");
    let heap = descriptors.heap.clone();
    // SAFETY: command owners outlive the one submitted list and its successful fence.
    let (allocator, list, fence) = unsafe {
        let allocator: ID3D12CommandAllocator = host
            .device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
            .unwrap();
        let list: ID3D12GraphicsCommandList = host
            .device
            .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
            .unwrap();
        let fence: ID3D12Fence = host.device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        (allocator, list, fence)
    };
    // SAFETY: exact native descriptors, current states, and in-range initialized copy footprints.
    unsafe {
        let cpu = heap.GetCPUDescriptorHandleForHeapStart();
        let stride = host
            .device
            .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
            as usize;
        let slot = |index| D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: cpu.ptr + index * stride,
        };
        let srv_words = [42, 8, 5768, 0, 0, 1, 0, 0, 0, 0];
        let uav_words = [42, 8, 0, 0, DEPTH, 0, 0, 0, 0, 0];
        descriptors.make_at(
            first,
            &View::Shader(input_owner.0 as usize, Some(srv_words)),
        );
        descriptors.make_at(
            first + 3,
            &View::Unordered(input_owner.0 as usize, 0, Some(uav_words)),
        );
        let raw_uav = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    NumElements: WORDS as u32,
                    Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                    ..Default::default()
                },
            },
        };
        for base in [first, first + 3] {
            descriptors.make_at(
                base + 1,
                &View::Unordered(output_owner.0 as usize, 0, Some(uav_words)),
            );
            host.device.CreateUnorderedAccessView(
                &raw,
                None,
                Some(&raw_uav),
                slot((base + 2) as usize),
            );
        }
        list.CopyTextureRegion(
            &subresource(&input),
            0,
            0,
            0,
            &placed(&upload, footprint, 0),
            None,
        );
        list.ResourceBarrier(&[transition(
            &input,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
        )]);
        list.SetDescriptorHeaps(&[Some(heap.clone())]);

        for (i, (root, pso)) in programs.iter().enumerate() {
            if i == 2 {
                list.ResourceBarrier(&[transition(
                    &input,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                )]);
            }
            let raw_state = if i == 0 {
                D3D12_RESOURCE_STATE_COMMON
            } else {
                D3D12_RESOURCE_STATE_COPY_SOURCE
            };
            list.ResourceBarrier(&[transition(&raw, raw_state, D3D12_RESOURCE_STATE_COPY_DEST)]);
            if i != 0 {
                list.ResourceBarrier(&[transition(
                    &output,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                )]);
            }
            list.CopyTextureRegion(
                &subresource(&output),
                0,
                0,
                0,
                &placed(&upload, footprint, volume_bytes),
                None,
            );
            list.CopyBufferRegion(&raw, 0, &upload, 2 * volume_bytes, RAW_BYTES);
            list.ResourceBarrier(&[
                transition(
                    &output,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
                transition(
                    &raw,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
            ]);
            list.SetComputeRootSignature(root);
            list.SetPipelineState(pso);
            // The translated descriptor table byte address uses a fixed 32-byte title stride.
            let table = (first + if cases[i].1 { 3 } else { 0 }) * 32;
            let mut constants = vec![0; shaders[i].user_data_dwords() as usize];
            for value in &shaders[i].user_data {
                constants[value.index as usize] = table;
            }
            for binding in &shaders[i].image_bindings {
                assert_eq!(binding.slot, 0);
                assert!(matches!(binding.dword, 0 | 8));
                constants[binding.word as usize] = if i % 2 == 0 {
                    leonida_shader::emit::IMAGE_CLASS_LEGACY
                } else {
                    let index = table / 32 + binding.dword / 8;
                    let class = descriptors.image_class(index, binding.uav);
                    assert_eq!(class, leonida_shader::emit::IMAGE_CLASS_R32_UINT);
                    class
                };
            }
            list.SetComputeRoot32BitConstants(
                0,
                constants.len() as u32,
                constants.as_ptr().cast(),
                0,
            );
            list.Dispatch(1, 1, 1);
            list.ResourceBarrier(&[
                transition(
                    &output,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                ),
                transition(
                    &raw,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                ),
            ]);
            list.CopyTextureRegion(
                &placed(&readback, footprint, i as u64 * volume_bytes),
                0,
                0,
                0,
                &subresource(&output),
                None,
            );
            list.CopyBufferRegion(
                &readback,
                readback_raw + i as u64 * RAW_BYTES,
                &raw,
                0,
                RAW_BYTES,
            );
        }
        list.Close().unwrap();
        host.queue()
            .ExecuteCommandLists(&[Some(list.cast().unwrap())]);
        if let Err(error) = host.queue().Signal(&fence, 1) {
            eprintln!("R32_UINT volume Signal failed: {error}");
            std::process::exit(3);
        }
    }
    wait_completed(&fence);
    drop(allocator);
    // Every recorded use retired. Rewrites and allocation release must clear the old class.
    use leonida_shader::emit::{IMAGE_CLASS_LEGACY, IMAGE_CLASS_R32_UINT};
    assert_eq!(descriptors.image_class(first, true), IMAGE_CLASS_LEGACY);
    assert_eq!(
        descriptors.image_class(first + 3, false),
        IMAGE_CLASS_LEGACY
    );
    let input_view = View::Shader(
        input_owner.0 as usize,
        Some([42, 8, 5768, 0, 0, 1, 0, 0, 0, 0]),
    );
    descriptors.make_null(first);
    assert_eq!(descriptors.image_class(first, false), IMAGE_CLASS_LEGACY);
    descriptors.make_at(first, &input_view);
    assert_eq!(descriptors.image_class(first, false), IMAGE_CLASS_R32_UINT);
    descriptors.make_at(first, &View::Shader(0, None));
    assert_eq!(descriptors.image_class(first, false), IMAGE_CLASS_LEGACY);
    descriptors.make_at(first, &input_view);
    assert_eq!(descriptors.image_class(first, false), IMAGE_CLASS_R32_UINT);
    descriptors.release(first, 6);
    for index in first..first + 6 {
        for uav in [false, true] {
            assert_eq!(descriptors.image_class(index, uav), IMAGE_CLASS_LEGACY);
        }
    }

    let mut pointer = std::ptr::null_mut();
    // SAFETY: the retained list completion proves every full copy into this range completed.
    let results: [([u32; WORDS], [u32; WORDS]); CASES] = unsafe {
        readback
            .Map(
                0,
                Some(&D3D12_RANGE {
                    Begin: 0,
                    End: readback_size as usize,
                }),
                Some(&mut pointer),
            )
            .unwrap();
        assert!(!pointer.is_null());
        let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), readback_size as usize);
        let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let data = std::array::from_fn(|i| {
            let load = std::array::from_fn(|n| {
                word((readback_raw + i as u64 * RAW_BYTES) as usize + n * 4) ^ LOAD_TAG
            });
            let store = std::array::from_fn(|n| {
                word(i * volume_bytes as usize + byte_offset(n, row_pitch))
            });
            (load, store)
        });
        readback.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
        data
    };
    eprintln!("uint-image store mode={store:?}");
    eprintln!("uint-image 3D shape={WIDTH}x{HEIGHT}x{DEPTH} words={WORDS} row_pitch={row_pitch}");
    eprintln!("uint-image expected load={expected_load:08x?}");
    eprintln!("uint-image expected store={expected_store:08x?}");
    for (i, ((load, store), (name, _))) in results.iter().zip(cases).enumerate() {
        eprintln!("uint-image {name} load={load:08x?}");
        eprintln!("uint-image {name} store={store:08x?}");
        let load_diff = load
            .iter()
            .zip(expected_load)
            .filter(|(a, b)| **a != *b)
            .count();
        let store_diff = store
            .iter()
            .zip(expected_store)
            .filter(|(a, b)| **a != *b)
            .count();
        eprintln!("uint-image {name} load_mismatches={load_diff} store_mismatches={store_diff}");
        assert!(!load.contains(&(u32::from_ne_bytes([SENTINEL; 4]) ^ LOAD_TAG)));
        assert!(!store.contains(&u32::from_ne_bytes([SENTINEL; 4])));
        if i % 2 != 0 {
            assert_eq!(*load, expected_load, "{name} load");
            assert_eq!(*store, expected_store, "{name} store");
        }
    }
}
