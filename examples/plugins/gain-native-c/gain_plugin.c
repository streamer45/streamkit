// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * StreamKit Gain Plugin - C Implementation
 *
 * A simple audio gain (volume) filter demonstrating how to write a native
 * plugin in pure C. This mirrors the functionality of the core audio::gain
 * node.
 *
 * The plugin applies a linear gain multiplier to incoming audio samples.
 * The gain parameter is tunable at runtime and renders as a slider in the UI.
 */

#include "streamkit_plugin.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ============================================================================
 * Configuration
 * ============================================================================ */

/** Default linear gain (1.0 = unity, no change) */
#define DEFAULT_GAIN 1.0f

/** Minimum allowed gain (0.0 = mute) */
#define MIN_GAIN 0.0f

/** Maximum allowed gain (4.0 = +12dB) */
#define MAX_GAIN 4.0f

/* ============================================================================
 * Plugin State
 * ============================================================================ */

typedef struct GainPluginState {
    float gain;              /**< Linear gain multiplier */
    CLogCallback log_cb;     /**< Logging callback */
    void* log_user_data;     /**< User data for logging */
} GainPluginState;

/* ============================================================================
 * Helper Functions
 * ============================================================================ */

/** Log a message using the host's logging callback */
static void plugin_log(GainPluginState* state, CLogLevel level, const char* msg) {
    if (state->log_cb) {
        state->log_cb(level, "gain_plugin_c", msg, state->log_user_data);
    }
}

/**
 * Parse gain from JSON parameters.
 *
 * This is a simple JSON parser that only handles the expected format:
 * {"gain": <number>}
 *
 * For production plugins, consider using a proper JSON library like cJSON.
 */
static float parse_gain(const char* json) {
    if (!json || json[0] == '\0') {
        return DEFAULT_GAIN;
    }

    /* Simple parsing: look for "gain" followed by : and a number */
    const char* key = strstr(json, "\"gain\"");
    if (!key) {
        return DEFAULT_GAIN;
    }

    /* Skip past the key and find the colon */
    const char* p = key + strlen("\"gain\"");
    while (*p && (*p == ' ' || *p == '\t' || *p == ':')) {
        p++;
    }

    /* Parse the number */
    char* end;
    float value = strtof(p, &end);
    if (end == p) {
        /* No number found */
        return DEFAULT_GAIN;
    }

    /* Clamp to valid range */
    if (value < MIN_GAIN) value = MIN_GAIN;
    if (value > MAX_GAIN) value = MAX_GAIN;

    return value;
}

/* ============================================================================
 * Plugin API Implementation
 * ============================================================================ */

/* Static metadata - must remain valid for plugin lifetime */
static const CAudioFormat g_audio_format = {
    .sample_rate = 0,       /* Wildcard - accepts any sample rate */
    .channels = 0,          /* Wildcard - accepts any number of channels */
    .sample_format = SAMPLE_FORMAT_F32
};

static const CPacketTypeInfo g_input_types[] = {
    {
        .type_discriminant = PACKET_TYPE_RAW_AUDIO,
        .audio_format = &g_audio_format
    }
};

static const CInputPin g_inputs[] = {
    {
        .name = "in",
        .accepts_types = g_input_types,
        .accepts_types_count = 1
    }
};

static const COutputPin g_outputs[] = {
    {
        .name = "out",
        .produces_type = {
            .type_discriminant = PACKET_TYPE_RAW_AUDIO,
            .audio_format = &g_audio_format
        }
    }
};

static const char* const g_categories[] = {"audio", "filters"};

static const CNodeMetadata g_metadata = {
    .kind = "gain_c",
    .description = "Audio gain (volume) filter implemented in C",
    .inputs = g_inputs,
    .inputs_count = 1,
    .outputs = g_outputs,
    .outputs_count = 1,
    .param_schema =
        "{"
        "  \"type\": \"object\","
        "  \"properties\": {"
        "    \"gain\": {"
        "      \"type\": \"number\","
        "      \"description\": \"Linear gain multiplier. 0.0 = mute, 1.0 = unity (no change), 2.0 = +6dB, 4.0 = +12dB\","
        "      \"default\": 1.0,"
        "      \"minimum\": 0.0,"
        "      \"maximum\": 4.0,"
        "      \"tunable\": true"
        "    }"
        "  }"
        "}",
    .categories = g_categories,
    .categories_count = 2
};

/** Get plugin metadata */
static const CNodeMetadata* gain_get_metadata(void) {
    return &g_metadata;
}

/** Create a new plugin instance */
static CPluginHandle gain_create_instance(const char* params,
                                          CLogCallback log_callback,
                                          void* log_user_data) {
    GainPluginState* state = malloc(sizeof(GainPluginState));
    if (!state) {
        return NULL;
    }

    state->log_cb = log_callback;
    state->log_user_data = log_user_data;

    /* Parse parameters */
    state->gain = parse_gain(params);

    char log_msg[128];
    snprintf(log_msg, sizeof(log_msg),
             "Created gain plugin instance: gain=%.4f",
             state->gain);
    plugin_log(state, LOG_LEVEL_INFO, log_msg);

    return (CPluginHandle)state;
}

/** Process an incoming packet */
static CResult gain_process_packet(CPluginHandle handle,
                                   const char* input_pin,
                                   const CPacket* packet,
                                   COutputCallback output_callback,
                                   void* callback_data) {
    (void)input_pin;  /* Unused - we only have one input pin */

    if (!handle) {
        return CResult_error("Null handle");
    }

    GainPluginState* state = (GainPluginState*)handle;

    /* Only process audio packets */
    if (packet->packet_type != PACKET_TYPE_RAW_AUDIO) {
        return CResult_error("Gain plugin only accepts audio packets");
    }

    const CAudioFrame* input_frame = (const CAudioFrame*)packet->data;
    if (!input_frame || !input_frame->samples) {
        return CResult_error("Invalid audio frame");
    }

    /* Allocate output samples buffer */
    float* output_samples = malloc(input_frame->sample_count * sizeof(float));
    if (!output_samples) {
        return CResult_error("Failed to allocate output buffer");
    }

    /* Apply gain to all samples */
    for (size_t i = 0; i < input_frame->sample_count; i++) {
        output_samples[i] = input_frame->samples[i] * state->gain;
    }

    /* Create output frame */
    CAudioFrame output_frame = {
        .sample_rate = input_frame->sample_rate,
        .channels = input_frame->channels,
        .samples = output_samples,
        .sample_count = input_frame->sample_count
    };

    /* Create output packet */
    CPacket output_packet = {
        .packet_type = PACKET_TYPE_RAW_AUDIO,
        .data = &output_frame,
        .len = sizeof(CAudioFrame)
    };

    /* Send output */
    CResult result = output_callback("out", &output_packet, callback_data);

    /* Clean up */
    free(output_samples);

    return result;
}

/** Update runtime parameters */
static CResult gain_update_params(CPluginHandle handle, const char* params) {
    if (!handle) {
        return CResult_error("Null handle");
    }

    GainPluginState* state = (GainPluginState*)handle;

    float old_gain = state->gain;
    state->gain = parse_gain(params);

    char log_msg[128];
    snprintf(log_msg, sizeof(log_msg),
             "Updated gain: %.4f -> %.4f",
             old_gain, state->gain);
    plugin_log(state, LOG_LEVEL_INFO, log_msg);

    return CResult_success();
}

/** Flush buffered data (no-op for this plugin) */
static CResult gain_flush(CPluginHandle handle,
                          COutputCallback output_callback,
                          void* callback_data) {
    (void)handle;
    (void)output_callback;
    (void)callback_data;
    /* Gain plugin doesn't buffer data, nothing to flush */
    return CResult_success();
}

/** Destroy plugin instance */
static void gain_destroy_instance(CPluginHandle handle) {
    if (handle) {
        GainPluginState* state = (GainPluginState*)handle;
        plugin_log(state, LOG_LEVEL_INFO, "Destroying gain plugin instance");
        free(state);
    }
}

/* ============================================================================
 * Plugin API Table
 * ============================================================================ */

static const CNativePluginAPI g_plugin_api = {
    .version = STREAMKIT_NATIVE_PLUGIN_API_VERSION,
    .get_metadata = gain_get_metadata,
    .create_instance = gain_create_instance,
    .process_packet = gain_process_packet,
    .update_params = gain_update_params,
    .flush = gain_flush,
    .destroy_instance = gain_destroy_instance
};

/* Export the plugin entry point */
STREAMKIT_PLUGIN_ENTRY(&g_plugin_api)
