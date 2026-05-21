// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! A test fixture that exports `streamkit_native_plugin_api` with an
//! out-of-range version (5).  Used to exercise the host's
//! version-mismatch error path.  All vtable functions are stubs that
//! must not be called by a correctly behaving host.

use std::os::raw::{c_char, c_void};

use streamkit_plugin_sdk_native::types::{
    CNativePluginAPI, CNodeCallbacks, CNodeMetadata, CPacket, CPluginHandle, CResult,
};

extern "C" fn stub_get_metadata() -> *const CNodeMetadata {
    std::ptr::null()
}

extern "C" fn stub_create_instance(
    _params: *const c_char,
    _log_cb: streamkit_plugin_sdk_native::types::CLogCallback,
    _log_user_data: *mut c_void,
) -> CPluginHandle {
    std::ptr::null_mut()
}

extern "C" fn stub_process_packet(
    _h: CPluginHandle,
    _pin: *const c_char,
    _pkt: *const CPacket,
    _cbs: *const CNodeCallbacks,
) -> CResult {
    CResult::error(c"unreachable".as_ptr())
}

extern "C" fn stub_update_params(_h: CPluginHandle, _p: *const c_char) -> CResult {
    CResult::error(c"unreachable".as_ptr())
}

extern "C" fn stub_flush(_h: CPluginHandle, _cbs: *const CNodeCallbacks) -> CResult {
    CResult::error(c"unreachable".as_ptr())
}

extern "C" fn stub_destroy_instance(_h: CPluginHandle) {}

static API: CNativePluginAPI = CNativePluginAPI {
    // Below MIN_SUPPORTED_API_VERSION (host should reject).
    version: 5,
    get_metadata: stub_get_metadata,
    create_instance: stub_create_instance,
    process_packet: stub_process_packet,
    update_params: stub_update_params,
    flush: stub_flush,
    destroy_instance: stub_destroy_instance,
    get_source_config: None,
    tick: None,
    get_runtime_param_schema: None,
    on_upstream_hint: None,
};

#[unsafe(no_mangle)]
pub extern "C" fn streamkit_native_plugin_api() -> *const CNativePluginAPI {
    &raw const API
}
