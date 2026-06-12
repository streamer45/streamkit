/*
 * SPDX-FileCopyrightText: © 2025 StreamKit Contributors
 *
 * SPDX-License-Identifier: MPL-2.0
 */

/**
 * StreamKit Gain Filter Plugin - C Implementation
 *
 * This plugin demonstrates how to write a basic audio processing node in C
 * using the WebAssembly Component Model with wit-bindgen.
 */

#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <stdio.h>
#include "plugin.h"

// Default configuration constants
#define DEFAULT_SAMPLE_RATE 48000
#define DEFAULT_CHANNELS 1
#define MIN_GAIN_DB -60.0f
#define MAX_GAIN_DB 20.0f

// Per-instance state structure
typedef struct {
    float gain_linear;
} gain_state_t;

// Helper function to clamp values
static float clamp_f32(float value, float min, float max) {
    if (value < min) return min;
    if (value > max) return max;
    return value;
}

// Helper function to convert dB to linear
static float db_to_linear(float db) {
    return powf(10.0f, db / 20.0f);
}

// Simple JSON parser to extract gain_db value
// Format expected: {"gain_db": <number>}
static float parse_gain_db(const char *json_str, size_t len) {
    if (!json_str || len == 0) return 0.0f;

    const char *gain_key = "\"gain_db\"";
    const char *pos = strstr(json_str, gain_key);
    if (!pos) return 0.0f;

    // Skip past the key and find the colon
    pos = strchr(pos, ':');
    if (!pos) return 0.0f;

    // Parse the number
    float value = 0.0f;
    if (sscanf(pos + 1, "%f", &value) == 1) {
        return value;
    }

    return 0.0f;
}

// Export metadata function
void exports_streamkit_plugin_node_metadata(exports_streamkit_plugin_node_node_metadata_t *ret) {
    exports_streamkit_plugin_node_node_metadata_t metadata;

    // Set kind (use dup because it will be freed by post_return)
    plugin_string_dup(&metadata.kind, "gain_filter_c");

    // Create input pin list
    metadata.inputs.len = 1;
    metadata.inputs.ptr = malloc(sizeof(streamkit_plugin_types_input_pin_t));

    plugin_string_dup(&metadata.inputs.ptr[0].name, "in");

    // Set accepted types (raw audio)
    metadata.inputs.ptr[0].accepts_types.len = 1;
    metadata.inputs.ptr[0].accepts_types.ptr = malloc(sizeof(streamkit_plugin_types_packet_type_t));
    metadata.inputs.ptr[0].accepts_types.ptr[0].tag = STREAMKIT_PLUGIN_TYPES_PACKET_TYPE_RAW_AUDIO;
    metadata.inputs.ptr[0].accepts_types.ptr[0].val.raw_audio.sample_rate = DEFAULT_SAMPLE_RATE;
    metadata.inputs.ptr[0].accepts_types.ptr[0].val.raw_audio.channels = DEFAULT_CHANNELS;
    metadata.inputs.ptr[0].accepts_types.ptr[0].val.raw_audio.sample_format = STREAMKIT_PLUGIN_TYPES_SAMPLE_FORMAT_FLOAT32;

    // Create output pin list
    metadata.outputs.len = 1;
    metadata.outputs.ptr = malloc(sizeof(streamkit_plugin_types_output_pin_t));

    plugin_string_dup(&metadata.outputs.ptr[0].name, "out");
    metadata.outputs.ptr[0].produces_type.tag = STREAMKIT_PLUGIN_TYPES_PACKET_TYPE_RAW_AUDIO;
    metadata.outputs.ptr[0].produces_type.val.raw_audio.sample_rate = DEFAULT_SAMPLE_RATE;
    metadata.outputs.ptr[0].produces_type.val.raw_audio.channels = DEFAULT_CHANNELS;
    metadata.outputs.ptr[0].produces_type.val.raw_audio.sample_format = STREAMKIT_PLUGIN_TYPES_SAMPLE_FORMAT_FLOAT32;

    // Set parameter schema
    const char *schema =
        "{"
        "  \"type\": \"object\","
        "  \"properties\": {"
        "    \"gain_db\": {"
        "      \"type\": \"number\","
        "      \"default\": 0.0,"
        "      \"description\": \"Gain in decibels (dB)\","
        "      \"minimum\": -60.0,"
        "      \"maximum\": 20.0"
        "    }"
        "  }"
        "}";
    plugin_string_dup(&metadata.param_schema, schema);

    // Set categories
    metadata.categories.len = 2;
    metadata.categories.ptr = malloc(2 * sizeof(plugin_string_t));
    plugin_string_dup(&metadata.categories.ptr[0], "audio");
    plugin_string_dup(&metadata.categories.ptr[1], "filters");

    *ret = metadata;
}

// Constructor
exports_streamkit_plugin_node_own_node_instance_t
exports_streamkit_plugin_node_constructor_node_instance(plugin_string_t *maybe_params) {
    gain_state_t *state = malloc(sizeof(gain_state_t));
    if (!state) {
        return (exports_streamkit_plugin_node_own_node_instance_t){0};
    }

    // Parse initial gain_db parameter
    float gain_db = 0.0f;
    if (maybe_params && maybe_params->ptr && maybe_params->len > 0) {
        gain_db = parse_gain_db((const char*)maybe_params->ptr, maybe_params->len);
        gain_db = clamp_f32(gain_db, MIN_GAIN_DB, MAX_GAIN_DB);
    }

    state->gain_linear = db_to_linear(gain_db);

    // Log initialization
    char log_buffer[256];
    snprintf(log_buffer, sizeof(log_buffer),
             "Gain filter instance constructed: %.2fdB (linear: %.3f)",
             gain_db, state->gain_linear);

    plugin_string_t log_msg;
    plugin_string_set(&log_msg, log_buffer);
    streamkit_plugin_host_log(STREAMKIT_PLUGIN_HOST_LOG_LEVEL_INFO, &log_msg);

    // Register the state as a Component Model resource and return the handle
    // This function registers the pointer with the resource table
    return exports_streamkit_plugin_node_node_instance_new((exports_streamkit_plugin_node_node_instance_t*)state);
}

// State kept alive across the async send-output subtask. The canonical ABI
// reads `args` lazily (while the subtask is STARTING) and writes `result`
// when it completes, so both must outlive the call.
typedef struct {
    streamkit_plugin_host_send_output_args_t args;
    streamkit_plugin_host_result_void_string_t result;
    plugin_waitable_set_t set;
} pending_send_t;

static plugin_callback_code_t finish_process(pending_send_t *pending) {
    exports_streamkit_plugin_node_result_void_string_t ret = {0};
    if (pending->result.is_err) {
        ret.is_err = true;
        ret.val.err = pending->result.val.err;
    }
    free(pending);
    exports_streamkit_plugin_node_method_node_instance_process_return(ret);
    return PLUGIN_CALLBACK_CODE_EXIT;
}

// Process function (async export: returns a callback code instead of a bool,
// reporting its result via process_return)
plugin_callback_code_t exports_streamkit_plugin_node_method_node_instance_process(
    exports_streamkit_plugin_node_borrow_node_instance_t self,
    plugin_string_t *input_pin,
    exports_streamkit_plugin_node_packet_t *packet) {

    (void)input_pin;
    gain_state_t *state = (gain_state_t*)(uintptr_t)self;

    // Debug log
    plugin_string_t debug_msg;
    char debug_buf[128];
    snprintf(debug_buf, sizeof(debug_buf), "[C] process() called, packet tag=%d", packet->tag);
    plugin_string_set(&debug_msg, debug_buf);
    streamkit_plugin_host_log(STREAMKIT_PLUGIN_HOST_LOG_LEVEL_DEBUG, &debug_msg);

    // Check if packet is audio
    if (packet->tag != STREAMKIT_PLUGIN_TYPES_PACKET_AUDIO) {
        exports_streamkit_plugin_node_result_void_string_t ret = {0};
        ret.is_err = true;
        plugin_string_dup(&ret.val.err, "Gain filter only accepts audio packets");
        exports_streamkit_plugin_node_method_node_instance_process_return(ret);
        return PLUGIN_CALLBACK_CODE_EXIT;
    }

    // Apply gain to all samples (in-place modification)
    streamkit_plugin_types_audio_frame_t *audio = &packet->val.audio;

    snprintf(debug_buf, sizeof(debug_buf), "[C] processing %zu samples, gain=%.3f",
             audio->samples.len, state->gain_linear);
    plugin_string_set(&debug_msg, debug_buf);
    streamkit_plugin_host_log(STREAMKIT_PLUGIN_HOST_LOG_LEVEL_DEBUG, &debug_msg);

    for (size_t i = 0; i < audio->samples.len; i++) {
        audio->samples.ptr[i] *= state->gain_linear;
    }

    plugin_string_set(&debug_msg, "[C] samples processed, sending output");
    streamkit_plugin_host_log(STREAMKIT_PLUGIN_HOST_LOG_LEVEL_DEBUG, &debug_msg);

    // Send output as an async subtask
    pending_send_t *pending = calloc(1, sizeof(pending_send_t));
    if (!pending) {
        exports_streamkit_plugin_node_result_void_string_t ret = {0};
        ret.is_err = true;
        plugin_string_dup(&ret.val.err, "Out of memory");
        exports_streamkit_plugin_node_method_node_instance_process_return(ret);
        return PLUGIN_CALLBACK_CODE_EXIT;
    }
    plugin_string_set(&pending->args.pin_name, "out");
    pending->args.packet = *packet;

    plugin_subtask_status_t status =
        streamkit_plugin_host_send_output(&pending->args, &pending->result);

    if (PLUGIN_SUBTASK_STATE(status) == PLUGIN_SUBTASK_RETURNED) {
        return finish_process(pending);
    }

    // The host hasn't completed the send yet: park the subtask in a waitable
    // set and resume from the callback when it completes.
    pending->set = plugin_waitable_set_new();
    plugin_waitable_join(PLUGIN_SUBTASK_HANDLE(status), pending->set);
    plugin_context_set_0(pending);
    return PLUGIN_CALLBACK_CODE_WAIT(pending->set);
}

plugin_callback_code_t exports_streamkit_plugin_node_method_node_instance_process_callback(
    plugin_event_t *event) {
    pending_send_t *pending = plugin_context_get_0();

    if (event->event != PLUGIN_EVENT_SUBTASK ||
        PLUGIN_SUBTASK_STATE(event->code) != PLUGIN_SUBTASK_RETURNED) {
        return PLUGIN_CALLBACK_CODE_WAIT(pending->set);
    }

    plugin_subtask_drop(event->waitable);
    plugin_waitable_set_drop(pending->set);
    return finish_process(pending);
}

// Update params function (async export, but completes synchronously)
plugin_callback_code_t exports_streamkit_plugin_node_method_node_instance_update_params(
    exports_streamkit_plugin_node_borrow_node_instance_t self,
    plugin_string_t *maybe_params) {

    gain_state_t *state = (gain_state_t*)(uintptr_t)self;
    exports_streamkit_plugin_node_result_void_string_t ok = {0};

    if (!maybe_params || !maybe_params->ptr || maybe_params->len == 0) {
        exports_streamkit_plugin_node_method_node_instance_update_params_return(ok);
        return PLUGIN_CALLBACK_CODE_EXIT;
    }

    float gain_db = parse_gain_db((const char*)maybe_params->ptr, maybe_params->len);
    gain_db = clamp_f32(gain_db, MIN_GAIN_DB, MAX_GAIN_DB);
    state->gain_linear = db_to_linear(gain_db);

    // Log update
    char log_buffer[256];
    snprintf(log_buffer, sizeof(log_buffer),
             "Gain updated via params: %.2fdB (linear: %.3f)",
             gain_db, state->gain_linear);

    plugin_string_t log_msg;
    plugin_string_set(&log_msg, log_buffer);
    streamkit_plugin_host_log(STREAMKIT_PLUGIN_HOST_LOG_LEVEL_INFO, &log_msg);

    exports_streamkit_plugin_node_method_node_instance_update_params_return(ok);
    return PLUGIN_CALLBACK_CODE_EXIT;
}

plugin_callback_code_t exports_streamkit_plugin_node_method_node_instance_update_params_callback(
    plugin_event_t *event) {
    (void)event;
    return PLUGIN_CALLBACK_CODE_EXIT;
}

// Cleanup function
void exports_streamkit_plugin_node_method_node_instance_cleanup(
    exports_streamkit_plugin_node_borrow_node_instance_t self) {

    gain_state_t *state = (gain_state_t*)(uintptr_t)self;

    // Log shutdown
    plugin_string_t log_msg;
    plugin_string_set(&log_msg, "Gain filter instance shutting down");
    streamkit_plugin_host_log(STREAMKIT_PLUGIN_HOST_LOG_LEVEL_INFO, &log_msg);

    free(state);
}

// Destructor function (required by Component Model resource lifecycle)
void exports_streamkit_plugin_node_node_instance_destructor(
    exports_streamkit_plugin_node_node_instance_t *rep) {
    // The cleanup is already handled by the cleanup() method above
    // This destructor is called when the resource handle is dropped
    // We don't need to free anything here since cleanup() does it
    (void)rep; // Unused parameter
}
