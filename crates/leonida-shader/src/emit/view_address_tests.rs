//! The address of a buffer view picked from the view heap, read from the
//! host's per-slot address table in `SLOT_STRIDES`.

use super::tests::{lower_with, root};
use crate::native::RootType;

const S_ENDPGM: u32 = 0xBF81_0000;

#[test]
fn view_address_switch_can_be_disabled_and_enabled_per_test() {
    let was = super::TEST_VIEW_ADDRS.replace(Some(false));
    assert!(!super::view_addrs_on());
    super::TEST_VIEW_ADDRS.set(Some(true));
    assert!(super::view_addrs_on());
    super::TEST_VIEW_ADDRS.set(was);
}

#[test]
fn a_view_picked_from_the_heap_for_its_address_reads_it_from_the_host() {
    // A buffer descriptor loaded from the heap (s0) at the slot a root
    // constant (s1) names, its address alone copied to LDS.
    let words = [
        0x8F02_8501, // s_lshl_b32 s2, s1, 5
        0xBE81_0380, // s_mov_b32 s1, 0
        0xF408_0100,
        0x0400_0000, // s_load_dwordx4 s[4:7], s[0:1], s2
        0x7E02_0204, // v_mov_b32 v1, s4
        0x7E04_0205, // v_mov_b32 v2, s5
        0xD834_0000,
        0x0000_0100, // ds_write_b32 v0, v1
        0xD834_0000,
        0x0000_0200, // ds_write_b32 v0, v2
        S_ENDPGM,
    ];
    let layout = vec![
        root(RootType::ResourceHeap, 0, 0),
        root(RootType::Constants, 1, 1),
    ];
    let was = super::TEST_VIEW_ADDRS.replace(Some(true));
    let l = lower_with(&words, layout, vec![]).unwrap();
    super::TEST_VIEW_ADDRS.set(was);
    assert!(l.root_srvs.contains(&super::super::shader::SLOT_STRIDES));
}
