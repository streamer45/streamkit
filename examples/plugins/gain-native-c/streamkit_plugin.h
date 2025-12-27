// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * StreamKit Native Plugin C ABI Header
 *
 * This header defines the stable C ABI for writing native plugins in C/C++.
 * Plugins must export a single symbol `streamkit_native_plugin_api` that
 * returns a pointer to a CNativePluginAPI struct.
 *
 * API Version: 2
 */

#ifndef STREAMKIT_PLUGIN_H
#define STREAMKIT_PLUGIN_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ============================================================================
 * Constants
 * ============================================================================ */

/** Current API version. Plugins and host check compatibility via this field. */
#define STREAMKIT_NATIVE_PLUGIN_API_VERSION 2

/* ============================================================================
 * Core Types
 * ============================================================================ */

/** Opaque handle to a plugin instance */
typedef void* CPluginHandle;

/** Log level for plugin logging */
typedef enum CLogLevel {
    LOG_LEVEL_TRACE = 0,
    LOG_LEVEL_DEBUG = 1,
    LOG_LEVEL_INFO = 2,
    LOG_LEVEL_WARN = 3,
    LOG_LEVEL_ERROR = 4
} CLogLevel;

/**
 * Callback function type for plugin logging.
 *
 * @param level     The log level
 * @param target    Module path (e.g., "gain_plugin_c")
 * @param message   The log message (null-terminated)
 * @param user_data Opaque pointer passed by host
 */
typedef void (*CLogCallback)(CLogLevel level, const char* target,
                             const char* message, void* user_data);

/** Result type for C ABI functions */
typedef struct CResult {
    bool success;
    const char* error_message;  /**< NULL on success, error string on failure */
} CResult;

/** Helper to create a successful result */
static inline CResult CResult_success(void) {
    CResult r = {true, NULL};
    return r;
}

/** Helper to create an error result */
static inline CResult CResult_error(const char* msg) {
    CResult r = {false, msg};
    return r;
}

/* ============================================================================
 * Audio Types
 * ============================================================================ */

/** Audio sample format */
typedef enum CSampleFormat {
    SAMPLE_FORMAT_F32 = 0,    /**< 32-bit IEEE 754 floating point */
    SAMPLE_FORMAT_S16LE = 1   /**< 16-bit signed integer, little-endian */
} CSampleFormat;

/** Audio format specification */
typedef struct CAudioFormat {
    uint32_t sample_rate;       /**< Samples per second (e.g., 16000, 48000) */
    uint16_t channels;          /**< Number of channels (1=mono, 2=stereo) */
    CSampleFormat sample_format;
} CAudioFormat;

/**
 * Audio frame data (for RawAudio packets).
 *
 * Samples are interleaved: [L, R, L, R, ...] for stereo.
 * The samples pointer is borrowed - do not free it.
 */
typedef struct CAudioFrame {
    uint32_t sample_rate;
    uint16_t channels;
    const float* samples;     /**< Array of f32 samples (read-only, borrowed) */
    size_t sample_count;      /**< Total number of samples across all channels */
} CAudioFrame;

/* ============================================================================
 * Packet Types
 * ============================================================================ */

/** Packet type discriminant */
typedef enum CPacketType {
    PACKET_TYPE_RAW_AUDIO = 0,
    PACKET_TYPE_OPUS_AUDIO = 1,
    PACKET_TYPE_TEXT = 2,
    PACKET_TYPE_TRANSCRIPTION = 3,
    PACKET_TYPE_CUSTOM = 4,
    PACKET_TYPE_BINARY = 5,
    PACKET_TYPE_ANY = 6,
    PACKET_TYPE_PASSTHROUGH = 7
} CPacketType;

/** Encoding for custom packets. */
typedef enum CCustomEncoding {
    CUSTOM_ENCODING_JSON = 0,
} CCustomEncoding;

/** Optional timing and sequencing metadata for packets. */
typedef struct CPacketMetadata {
    uint64_t timestamp_us;
    bool has_timestamp_us;
    uint64_t duration_us;
    bool has_duration_us;
    uint64_t sequence;
    bool has_sequence;
} CPacketMetadata;

/**
 * Custom packet payload passed across the ABI boundary.
 *
 * data_json points to UTF-8 encoded JSON (not null-terminated).
 */
typedef struct CCustomPacket {
    const char* type_id;               /**< Null-terminated type id string */
    CCustomEncoding encoding;          /**< Currently only JSON */
    const uint8_t* data_json;          /**< UTF-8 JSON bytes */
    size_t data_len;                   /**< Byte length of data_json */
    const CPacketMetadata* metadata;   /**< Optional (can be NULL) */
} CCustomPacket;

/**
 * Full packet type with optional format information.
 * For RawAudio, includes the audio format details.
 */
typedef struct CPacketTypeInfo {
    CPacketType type_discriminant;
    const CAudioFormat* audio_format;  /**< Non-NULL only for RawAudio */
    const char* custom_type_id;        /**< Non-NULL only for Custom */
} CPacketTypeInfo;

/**
 * Generic packet container.
 *
 * Data interpretation depends on packet_type:
 * - RawAudio:      data points to CAudioFrame, len is sizeof(CAudioFrame)
 * - Text:          data is null-terminated C string, len includes null
 * - Transcription: data is JSON bytes, len is byte count
 * - Custom:        data points to CCustomPacket, len is sizeof(CCustomPacket)
 * - Binary:        data is raw bytes, len is byte count
 */
typedef struct CPacket {
    CPacketType packet_type;
    const void* data;
    size_t len;
} CPacket;

/* ============================================================================
 * Pin Definitions
 * ============================================================================ */

/** Input pin definition */
typedef struct CInputPin {
    const char* name;                       /**< Pin name (e.g., "in") */
    const CPacketTypeInfo* accepts_types;   /**< Array of accepted types */
    size_t accepts_types_count;
} CInputPin;

/** Output pin definition */
typedef struct COutputPin {
    const char* name;                       /**< Pin name (e.g., "out") */
    CPacketTypeInfo produces_type;          /**< Single type produced */
} COutputPin;

/** Node metadata returned by plugin */
typedef struct CNodeMetadata {
    const char* kind;                       /**< Plugin name (e.g., "gain_c") */
    const char* description;                /**< Optional description (can be NULL) */
    const CInputPin* inputs;
    size_t inputs_count;
    const COutputPin* outputs;
    size_t outputs_count;
    const char* param_schema;               /**< JSON Schema as string */
    const char* const* categories;          /**< Array of category strings */
    size_t categories_count;
} CNodeMetadata;

/* ============================================================================
 * Callbacks
 * ============================================================================ */

/**
 * Callback function for sending output packets.
 *
 * @param pin_name      Output pin name (null-terminated)
 * @param packet        Packet to send
 * @param user_data     Opaque pointer from host
 * @return              CResult indicating success or failure
 */
typedef CResult (*COutputCallback)(const char* pin_name, const CPacket* packet,
                                   void* user_data);

/* ============================================================================
 * Plugin API Structure
 * ============================================================================ */

/**
 * The main plugin API structure.
 *
 * Plugins export a function `streamkit_native_plugin_api()` that returns
 * a pointer to this struct. All function pointers must be non-NULL.
 */
typedef struct CNativePluginAPI {
    /** API version for compatibility checking. Must be STREAMKIT_NATIVE_PLUGIN_API_VERSION */
    uint32_t version;

    /**
     * Get metadata about the node type.
     * @return Pointer to CNodeMetadata (must remain valid for plugin lifetime)
     */
    const CNodeMetadata* (*get_metadata)(void);

    /**
     * Create a new plugin instance.
     *
     * @param params        JSON string with initialization parameters (can be NULL)
     * @param log_callback  Callback for plugin to send log messages to host
     * @param log_user_data Opaque pointer to pass to log callback
     * @return              Opaque handle to the instance, or NULL on error
     */
    CPluginHandle (*create_instance)(const char* params, CLogCallback log_callback,
                                     void* log_user_data);

    /**
     * Process an incoming packet.
     *
     * @param handle          Plugin instance handle
     * @param input_pin       Name of the input pin
     * @param packet          The packet to process
     * @param output_callback Callback to send output packets
     * @param callback_data   User data to pass to callback
     * @return                CResult indicating success or failure
     */
    CResult (*process_packet)(CPluginHandle handle, const char* input_pin,
                              const CPacket* packet, COutputCallback output_callback,
                              void* callback_data);

    /**
     * Update runtime parameters.
     *
     * @param handle Plugin instance handle
     * @param params JSON string with new parameters (can be NULL)
     * @return       CResult indicating success or failure
     */
    CResult (*update_params)(CPluginHandle handle, const char* params);

    /**
     * Flush any buffered data when input stream ends.
     *
     * @param handle          Plugin instance handle
     * @param output_callback Callback to send output packets
     * @param callback_data   User data to pass to callback
     * @return                CResult indicating success or failure
     */
    CResult (*flush)(CPluginHandle handle, COutputCallback output_callback,
                     void* callback_data);

    /**
     * Destroy a plugin instance.
     *
     * @param handle Plugin instance handle
     */
    void (*destroy_instance)(CPluginHandle handle);
} CNativePluginAPI;

/* ============================================================================
 * Plugin Entry Point
 * ============================================================================ */

/**
 * Symbol that plugins must export.
 *
 * The host will load the plugin library and look up this symbol to get
 * a pointer to the plugin's CNativePluginAPI structure.
 */
#define STREAMKIT_PLUGIN_EXPORT __attribute__((visibility("default")))

/**
 * Macro to define the plugin entry point.
 *
 * Usage:
 *   STREAMKIT_PLUGIN_ENTRY(my_plugin_api)
 *
 * where `my_plugin_api` is your CNativePluginAPI struct.
 */
#define STREAMKIT_PLUGIN_ENTRY(api_ptr)                                    \
    STREAMKIT_PLUGIN_EXPORT                                                \
    const CNativePluginAPI* streamkit_native_plugin_api(void) {            \
        return (api_ptr);                                                  \
    }

#ifdef __cplusplus
}
#endif

#endif /* STREAMKIT_PLUGIN_H */
