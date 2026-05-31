// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
#[allow(clippy::unwrap_used)]
fn test_self_reference_needs_rejected() {
    let yaml = r"
mode: dynamic
nodes:
  peer:
    kind: test_node
    params: {}
    needs: peer
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let result = compile(user_pipeline);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("Circular dependency"), "Error should mention circular dependency: {err}");
    assert!(err.contains("peer"), "Error should mention the node name: {err}");
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_circular_needs_rejected() {
    let yaml = r"
mode: dynamic
nodes:
  node_a:
    kind: test_node
    needs: node_b
  node_b:
    kind: test_node
    needs: node_a
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let result = compile(user_pipeline);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("Circular dependency"), "Error should mention circular dependency: {err}");
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_invalid_needs_reference() {
    let yaml = r"
mode: dynamic
nodes:
  node_a:
    kind: test_node
    needs: non_existent_node
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let result = compile(user_pipeline);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("node_a"));
    assert!(err.contains("non_existent_node"));
    assert!(err.contains("needs"));
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_bidirectional_transport_not_flagged_as_cycle() {
    let yaml = r"
mode: dynamic
nodes:
  file_reader:
    kind: core::file_reader
    params:
      path: /tmp/test.opus
  ogg_demuxer:
    kind: containers::ogg::demuxer
    needs: file_reader
  pacer:
    kind: core::pacer
    needs: ogg_demuxer
  moq_publisher:
    kind: transport::moq::publisher
    params:
      broadcast: input
    needs: pacer
  moq_peer:
    kind: transport::moq::peer
    params:
      input_broadcasts:
        - input
      output_broadcast: output
  ogg_muxer:
    kind: containers::ogg::muxer
    needs:
      in: moq_peer.audio/data
  file_writer:
    kind: core::file_writer
    params:
      path: /tmp/output.opus
    needs: ogg_muxer
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let result = compile(user_pipeline);

    assert!(
        result.is_ok(),
        "Bidirectional transport pattern should not be flagged as a cycle: {:?}",
        result.err()
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_bidirectional_cycle_allowed() {
    let yaml = r"
mode: dynamic
nodes:
  decoder:
    kind: audio::opus::decoder
    needs:
      in: moq_peer.audio/data
  encoder:
    kind: audio::opus::encoder
    needs: mixer
  gain:
    kind: audio::gain
    needs: decoder
  mixer:
    kind: audio::mixer
    needs: gain
  moq_peer:
    kind: transport::moq::peer
    params:
      input_broadcasts:
        - input
      output_broadcast: output
    needs: encoder
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let result = compile(user_pipeline);

    assert!(result.is_ok(), "Cycle with bidirectional node should be allowed: {:?}", result.err());
}

#[test]
fn test_sample_moq_mixing_compiles() {
    let yaml = include_str!("../../../../samples/pipelines/dynamic/moq_mixing.yml");
    let user_pipeline = parse_yaml(yaml).unwrap();
    let result = compile(user_pipeline);

    assert!(result.is_ok(), "Sample pipeline moq_mixing.yml should compile: {:?}", result.err());
}

#[test]
fn test_sample_moq_aac_mixing_compiles() {
    let yaml = include_str!("../../../../samples/pipelines/dynamic/moq_aac_mixing.yml");
    let user_pipeline = parse_yaml(yaml).unwrap();
    let result = compile(user_pipeline);

    assert!(
        result.is_ok(),
        "Sample pipeline moq_aac_mixing.yml should compile: {:?}",
        result.err()
    );
}

/// Every shipped dynamic sample must parse and compile (structural validation:
/// YAML schema, `needs` references, pin resolution, cycle rules). This guards
/// against drift between the samples and the YAML/compiler contract — node
/// *availability* (feature flags, plugins) is checked separately against a live
/// registry, since the compiler is registry-agnostic.
#[test]
// Fixture-traversal unwraps should panic and identify the offending sample.
#[allow(clippy::unwrap_used)]
fn test_all_dynamic_samples_parse_and_compile() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../samples/pipelines/dynamic");
    let mut checked = 0usize;
    let mut failures = Vec::new();

    for entry in std::fs::read_dir(dir).expect("dynamic samples directory should exist") {
        let path = entry.unwrap().path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "yml" && ext != "yaml" {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let yaml = std::fs::read_to_string(&path).unwrap();
        checked += 1;

        match parse_yaml(&yaml) {
            Ok(pipeline) => {
                if let Err(e) = compile(pipeline) {
                    failures.push(format!("{name}: compile failed: {e}"));
                }
            },
            Err(e) => failures.push(format!("{name}: parse failed: {e}")),
        }
    }

    assert!(checked > 0, "expected to find dynamic sample files in {dir}");
    assert!(
        failures.is_empty(),
        "{} dynamic sample(s) failed to parse/compile:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
#[allow(clippy::unwrap_used, clippy::expect_used)]
fn test_multiple_inputs_numbered_pins() {
    let yaml = r"
mode: dynamic
nodes:
  input_a:
    kind: test_source
  input_b:
    kind: test_source
  mixer:
    kind: audio::mixer
    needs:
    - input_a
    - input_b
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let pipeline = compile(user_pipeline).unwrap();

    // Should have 3 nodes
    assert_eq!(pipeline.nodes.len(), 3);

    // Should have 2 connections
    assert_eq!(pipeline.connections.len(), 2);

    // First connection should use in_0
    let conn_a = pipeline
        .connections
        .iter()
        .find(|c| c.from_node == "input_a")
        .expect("Should have connection from input_a");
    assert_eq!(conn_a.to_node, "mixer");
    assert_eq!(conn_a.from_pin, "out");
    assert_eq!(conn_a.to_pin, "in_0");

    // Second connection should use in_1
    let conn_b = pipeline
        .connections
        .iter()
        .find(|c| c.from_node == "input_b")
        .expect("Should have connection from input_b");
    assert_eq!(conn_b.to_node, "mixer");
    assert_eq!(conn_b.from_pin, "out");
    assert_eq!(conn_b.to_pin, "in_1");
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_single_input_uses_in_pin() {
    let yaml = r"
mode: dynamic
nodes:
  source:
    kind: test_source
  sink:
    kind: test_sink
    needs: source
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let pipeline = compile(user_pipeline).unwrap();

    // Should have 2 nodes
    assert_eq!(pipeline.nodes.len(), 2);

    // Should have 1 connection
    assert_eq!(pipeline.connections.len(), 1);

    // Single connection should use "in" (not "in_0")
    let conn = &pipeline.connections[0];
    assert_eq!(conn.from_node, "source");
    assert_eq!(conn.to_node, "sink");
    assert_eq!(conn.from_pin, "out");
    assert_eq!(conn.to_pin, "in");
}

#[test]
#[allow(clippy::unwrap_used, clippy::expect_used)]
fn test_mixer_auto_configures_num_inputs() {
    let yaml = r"
mode: oneshot
nodes:
  input_a:
    kind: test_source
  input_b:
    kind: test_source
  mixer:
    kind: audio::mixer
    params:
      # num_inputs intentionally omitted
    needs:
    - input_a
    - input_b
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let pipeline = compile(user_pipeline).unwrap();

    // The mixer node should have num_inputs automatically set to 2 (oneshot mode)
    let mixer_node = pipeline.nodes.get("mixer").expect("mixer node should exist");
    assert_eq!(mixer_node.kind, "audio::mixer");

    // Extract num_inputs from params
    if let Some(serde_json::Value::Object(ref map)) = mixer_node.params {
        let num_inputs_value = map.get("num_inputs").expect("num_inputs should be set");
        if let serde_json::Value::Number(n) = num_inputs_value {
            assert_eq!(n.as_u64(), Some(2));
        } else {
            panic!("num_inputs should be a number");
        }
    } else {
        panic!("mixer params should be an object");
    }
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_steps_format_compilation() {
    let yaml = r"
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: audio::gain
    params:
      gain: 2.0
  - kind: streamkit::http_output
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let pipeline = compile(user_pipeline).unwrap();

    // Should have 3 nodes with generated names
    assert_eq!(pipeline.nodes.len(), 3);
    assert!(pipeline.nodes.contains_key("step_0"));
    assert!(pipeline.nodes.contains_key("step_1"));
    assert!(pipeline.nodes.contains_key("step_2"));

    // Should have 2 connections (linear chain)
    assert_eq!(pipeline.connections.len(), 2);

    // First connection: step_0 -> step_1
    let conn0 = &pipeline.connections[0];
    assert_eq!(conn0.from_node, "step_0");
    assert_eq!(conn0.to_node, "step_1");
    assert_eq!(conn0.from_pin, "out");
    assert_eq!(conn0.to_pin, "in");

    // Second connection: step_1 -> step_2
    let conn1 = &pipeline.connections[1];
    assert_eq!(conn1.from_node, "step_1");
    assert_eq!(conn1.to_node, "step_2");

    // Verify params preserved
    let gain_node = pipeline.nodes.get("step_1").unwrap();
    assert!(gain_node.params.is_some());
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_mode_preservation() {
    // Test OneShot mode
    let yaml_oneshot = r"
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: streamkit::http_output
";
    let pipeline = parse_yaml(yaml_oneshot).unwrap();
    let compiled = compile(pipeline).unwrap();
    assert_eq!(compiled.mode, EngineMode::OneShot);

    // Test Dynamic mode
    let yaml_dynamic = r"
mode: dynamic
steps:
  - kind: core::passthrough
";
    let pipeline = parse_yaml(yaml_dynamic).unwrap();
    let compiled = compile(pipeline).unwrap();
    assert_eq!(compiled.mode, EngineMode::Dynamic);
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_default_mode_is_dynamic() {
    let yaml = r"
# mode not specified
steps:
  - kind: core::passthrough
";
    let pipeline = parse_yaml(yaml).unwrap();
    let compiled = compile(pipeline).unwrap();
    assert_eq!(compiled.mode, EngineMode::Dynamic);
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_name_and_description_preservation() {
    let yaml = r"
name: Test Pipeline
description: A test pipeline for validation
mode: dynamic
steps:
  - kind: core::passthrough
";
    let pipeline = parse_yaml(yaml).unwrap();
    let compiled = compile(pipeline).unwrap();

    assert_eq!(compiled.name, Some("Test Pipeline".to_string()));
    assert_eq!(compiled.description, Some("A test pipeline for validation".to_string()));
}

#[test]
#[allow(clippy::unwrap_used, clippy::expect_used)]
fn test_connection_mode_in_needs() {
    let yaml = r"
mode: dynamic
nodes:
  source:
    kind: test_source
  main_sink:
    kind: test_sink
    needs: source
  metrics:
    kind: test_metrics
    needs:
      node: source
      mode: best_effort
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let pipeline = compile(user_pipeline).unwrap();

    // Should have 3 nodes
    assert_eq!(pipeline.nodes.len(), 3);

    // Should have 2 connections
    assert_eq!(pipeline.connections.len(), 2);

    // Connection to main_sink should be Reliable (default)
    let main_conn = pipeline
        .connections
        .iter()
        .find(|c| c.to_node == "main_sink")
        .expect("Should have connection to main_sink");
    assert_eq!(main_conn.mode, ConnectionMode::Reliable);

    // Connection to metrics should be BestEffort
    let metrics_conn = pipeline
        .connections
        .iter()
        .find(|c| c.to_node == "metrics")
        .expect("Should have connection to metrics");
    assert_eq!(metrics_conn.mode, ConnectionMode::BestEffort);
}

#[test]
#[allow(clippy::unwrap_used, clippy::expect_used)]
fn test_connection_mode_in_needs_list() {
    let yaml = r"
mode: dynamic
nodes:
  input_a:
    kind: test_source
  input_b:
    kind: test_source
  mixer:
    kind: audio::mixer
    needs:
      - input_a
      - node: input_b
        mode: best_effort
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let pipeline = compile(user_pipeline).unwrap();

    // Should have 3 nodes
    assert_eq!(pipeline.nodes.len(), 3);

    // Should have 2 connections
    assert_eq!(pipeline.connections.len(), 2);

    // Connection from input_a should be Reliable (default, simple string syntax)
    let conn_a = pipeline
        .connections
        .iter()
        .find(|c| c.from_node == "input_a")
        .expect("Should have connection from input_a");
    assert_eq!(conn_a.mode, ConnectionMode::Reliable);
    assert_eq!(conn_a.to_pin, "in_0");

    // Connection from input_b should be BestEffort (object syntax)
    let conn_b = pipeline
        .connections
        .iter()
        .find(|c| c.from_node == "input_b")
        .expect("Should have connection from input_b");
    assert_eq!(conn_b.mode, ConnectionMode::BestEffort);
    assert_eq!(conn_b.to_pin, "in_1");
}

#[test]
#[allow(clippy::unwrap_used, clippy::expect_used)]
fn test_needs_map_explicit_pin_targeting() {
    let yaml = r"
mode: dynamic
nodes:
  vp9_encoder:
    kind: video::vp9_encoder
  opus_encoder:
    kind: audio::opus_encoder
  muxer:
    kind: containers::webm_muxer
    needs:
      video: vp9_encoder
      audio: opus_encoder
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let pipeline = compile(user_pipeline).unwrap();

    // Should have 3 nodes
    assert_eq!(pipeline.nodes.len(), 3);

    // Should have 2 connections
    assert_eq!(pipeline.connections.len(), 2);

    // Connection from vp9_encoder should target the "video" pin
    let video_conn = pipeline
        .connections
        .iter()
        .find(|c| c.from_node == "vp9_encoder")
        .expect("Should have connection from vp9_encoder");
    assert_eq!(video_conn.to_node, "muxer");
    assert_eq!(video_conn.to_pin, "video");
    assert_eq!(video_conn.from_pin, "out");

    // Connection from opus_encoder should target the "audio" pin
    let audio_conn = pipeline
        .connections
        .iter()
        .find(|c| c.from_node == "opus_encoder")
        .expect("Should have connection from opus_encoder");
    assert_eq!(audio_conn.to_node, "muxer");
    assert_eq!(audio_conn.to_pin, "audio");
    assert_eq!(audio_conn.from_pin, "out");
}

#[test]
#[allow(clippy::unwrap_used, clippy::expect_used)]
fn test_needs_map_with_output_pin_specifier() {
    let yaml = r"
mode: dynamic
nodes:
  source:
    kind: test_source
  sink:
    kind: test_sink
    needs:
      my_input: source.alt_out
";

    let user_pipeline = parse_yaml(yaml).unwrap();
    let pipeline = compile(user_pipeline).unwrap();

    assert_eq!(pipeline.connections.len(), 1);
    let conn = &pipeline.connections[0];
    assert_eq!(conn.from_node, "source");
    assert_eq!(conn.from_pin, "alt_out");
    assert_eq!(conn.to_node, "sink");
    assert_eq!(conn.to_pin, "my_input");
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_client_section_parsed_in_steps() {
    let yaml = r#"
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: streamkit::http_output
client:
  input:
    type: file_upload
    accept: "audio/*"
  output:
    type: transcription
"#;
    let pipeline = parse_yaml(yaml).unwrap();
    let compiled = compile(pipeline).unwrap();

    let client = compiled.client.expect("client section should be present");
    let input = client.input.expect("input config should be present");
    assert!(matches!(input.input_type, InputType::FileUpload));
    assert_eq!(input.accept.as_deref(), Some("audio/*"));

    let output = client.output.expect("output config should be present");
    assert!(matches!(output.output_type, OutputType::Transcription));
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_client_section_parsed_in_dag() {
    let yaml = r#"
mode: dynamic
nodes:
  peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq/test
      input_broadcasts:
        - camera
      output_broadcast: output
client:
  gateway_path: /moq/test
  publish:
    broadcast: camera
    tracks:
      - kind: audio
        source: microphone
      - kind: video
        source: camera
  watch:
    broadcast: output
    audio: true
    video: true
"#;
    let pipeline = parse_yaml(yaml).unwrap();
    let compiled = compile(pipeline).unwrap();

    let client = compiled.client.expect("client section should be present");
    assert_eq!(client.gateway_path.as_deref(), Some("/moq/test"));

    let publish = client.publish.expect("publish config should be present");
    assert_eq!(publish.broadcast, "camera");
    assert_eq!(publish.tracks.len(), 2);
    assert_eq!(publish.tracks[0].kind, TrackKind::Audio);
    assert_eq!(publish.tracks[0].source, CaptureSource::Microphone);
    assert_eq!(publish.tracks[1].kind, TrackKind::Video);
    assert_eq!(publish.tracks[1].source, CaptureSource::Camera);

    let watch = client.watch.expect("watch config should be present");
    assert_eq!(watch.broadcast.as_deref(), Some("output"));
    assert!(watch.audio);
    assert!(watch.video);
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_client_section_optional() {
    let yaml = r"
mode: oneshot
steps:
  - kind: core::passthrough
";
    let pipeline = parse_yaml(yaml).unwrap();
    let compiled = compile(pipeline).unwrap();
    assert!(compiled.client.is_none());
}

#[test]
fn test_invalid_client_section_rejected() {
    let yaml = r#"
mode: oneshot
steps:
  - kind: streamkit::http_input
client:
  input:
    type: invalid_type
"#;
    let result = parse_yaml(yaml);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("Invalid client section"), "Error should mention client section: {err}");
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_client_section_with_field_hints() {
    let yaml = r#"
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: streamkit::http_output
client:
  input:
    type: file_upload
    accept: "audio/*"
    field_hints:
      text:
        type: text
        placeholder: "Enter your prompt"
      reference:
        type: file
        accept: "audio/*"
  output:
    type: audio
"#;
    let pipeline = parse_yaml(yaml).unwrap();
    let compiled = compile(pipeline).unwrap();

    let client = compiled.client.expect("client section should be present");
    let input = client.input.expect("input config should be present");
    let hints = input.field_hints.expect("field_hints should be present");

    assert_eq!(hints.len(), 2);

    let text_hint = hints.get("text").expect("text hint should exist");
    assert!(matches!(text_hint.field_type, Some(FieldType::Text)));
    assert_eq!(text_hint.placeholder.as_deref(), Some("Enter your prompt"));

    let ref_hint = hints.get("reference").expect("reference hint should exist");
    assert!(matches!(ref_hint.field_type, Some(FieldType::File)));
    assert_eq!(ref_hint.accept.as_deref(), Some("audio/*"));
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_client_section_with_asset_tags() {
    let yaml = r#"
mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: streamkit::http_output
client:
  input:
    type: file_upload
    accept: "audio/*"
    asset_tags:
      - speech
      - voice
  output:
    type: transcription
"#;
    let pipeline = parse_yaml(yaml).unwrap();
    let compiled = compile(pipeline).unwrap();

    let client = compiled.client.expect("client section should be present");
    let input = client.input.expect("input config should be present");
    let tags = input.asset_tags.expect("asset_tags should be present");
    assert_eq!(tags, vec!["speech", "voice"]);
}

fn dynamic_client() -> ClientSection {
    ClientSection {
        relay_url: None,
        gateway_path: Some("/moq/test".into()),
        publish: Some(PublishConfig {
            broadcast: "input".into(),
            tracks: vec![PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            }],
        }),
        watch: Some(WatchConfig {
            broadcast: Some("output".into()),
            mse_path: None,
            audio: true,
            video: true,
        }),
        input: None,
        output: None,
        ..Default::default()
    }
}

fn oneshot_client() -> ClientSection {
    ClientSection {
        relay_url: None,
        gateway_path: None,
        publish: None,
        watch: None,
        input: Some(InputConfig {
            input_type: InputType::FileUpload,
            accept: Some("audio/*".into()),
            asset_tags: None,
            placeholder: None,
            field_hints: None,
        }),
        output: Some(OutputConfig { output_type: OutputType::Audio }),
        ..Default::default()
    }
}

#[test]
fn test_lint_clean_dynamic() {
    let warnings = lint_client_section(&dynamic_client(), EngineMode::Dynamic);
    assert!(warnings.is_empty(), "Expected no warnings: {warnings:?}");
}

#[test]
fn test_lint_clean_oneshot() {
    let warnings = lint_client_section(&oneshot_client(), EngineMode::OneShot);
    assert!(warnings.is_empty(), "Expected no warnings: {warnings:?}");
}

#[test]
fn test_lint_mode_mismatch_dynamic_with_oneshot_fields() {
    let mut c = dynamic_client();
    c.input = Some(InputConfig {
        input_type: InputType::FileUpload,
        accept: None,
        asset_tags: None,
        placeholder: None,
        field_hints: None,
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(warnings.iter().any(|w| w.rule == "mode-mismatch-dynamic"));
}

#[test]
fn test_lint_mode_mismatch_oneshot_with_dynamic_fields() {
    let mut c = oneshot_client();
    c.gateway_path = Some("/moq/test".into());
    let warnings = lint_client_section(&c, EngineMode::OneShot);
    assert!(warnings.iter().any(|w| w.rule == "mode-mismatch-oneshot"));
}

#[test]
fn test_lint_mode_mismatch_oneshot_with_controls() {
    let mut c = oneshot_client();
    c.controls = Some(vec![ControlConfig {
        label: "Toggle".into(),
        control_type: ControlType::Toggle,
        node: "some_node".into(),
        property: "enabled".into(),
        group: None,
        default: None,
        min: None,
        max: None,
        step: None,
        value: None,
        options: None,
    }]);
    let warnings = lint_client_section(&c, EngineMode::OneShot);
    assert!(warnings.iter().any(|w| w.rule == "mode-mismatch-oneshot"));
}

#[test]
fn test_lint_missing_gateway() {
    let c = ClientSection {
        relay_url: None,
        gateway_path: None,
        publish: Some(PublishConfig {
            broadcast: "x".into(),
            tracks: vec![PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            }],
        }),
        watch: None,
        input: None,
        output: None,
        ..Default::default()
    };
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(warnings.iter().any(|w| w.rule == "missing-gateway"));
}

#[test]
fn test_lint_empty_tracks() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig { broadcast: "x".into(), tracks: vec![] });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(warnings.iter().any(|w| w.rule == "empty-tracks"));
}

#[test]
fn test_lint_watch_no_media() {
    let mut c = dynamic_client();
    c.watch = Some(WatchConfig {
        broadcast: Some("x".into()),
        mse_path: None,
        audio: false,
        video: false,
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(warnings.iter().any(|w| w.rule == "watch-no-media"));
}

#[test]
fn test_lint_empty_broadcast() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: String::new(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Audio,
            source: CaptureSource::Microphone,
            broadcast: None,
            width: None,
            height: None,
            codec: None,
            max_bitrate: None,
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(warnings.iter().any(|w| w.rule == "empty-broadcast"));
}

#[test]
fn test_lint_duplicate_broadcast() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "same".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Audio,
            source: CaptureSource::Microphone,
            broadcast: None,
            width: None,
            height: None,
            codec: None,
            max_bitrate: None,
        }],
    });
    c.watch = Some(WatchConfig {
        broadcast: Some("same".into()),
        mse_path: None,
        audio: true,
        video: true,
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(warnings.iter().any(|w| w.rule == "duplicate-broadcast"));
}

#[test]
fn test_lint_duplicate_broadcast_track_override() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![
            PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
            PublishTrackConfig {
                kind: TrackKind::Video,
                source: CaptureSource::Camera,
                broadcast: Some("output".into()), // overrides to match watch
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
        ],
    });
    c.watch = Some(WatchConfig {
        broadcast: Some("output".into()),
        mse_path: None,
        audio: true,
        video: true,
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        warnings.iter().any(|w| w.rule == "duplicate-broadcast"),
        "Should warn when a track-level broadcast override matches watch.broadcast: {warnings:?}"
    );
}

#[test]
fn test_lint_input_none_with_accept() {
    let c = ClientSection {
        relay_url: None,
        gateway_path: None,
        publish: None,
        watch: None,
        input: Some(InputConfig {
            input_type: InputType::None,
            accept: Some("audio/*".into()),
            asset_tags: None,
            placeholder: None,
            field_hints: None,
        }),
        output: Some(OutputConfig { output_type: OutputType::Video }),
        ..Default::default()
    };
    let warnings = lint_client_section(&c, EngineMode::OneShot);
    assert!(warnings.iter().any(|w| w.rule == "input-none-with-accept"));
}

#[test]
fn test_lint_input_trigger_with_accept() {
    let c = ClientSection {
        relay_url: None,
        gateway_path: None,
        publish: None,
        watch: None,
        input: Some(InputConfig {
            input_type: InputType::Trigger,
            accept: Some("audio/*".into()),
            asset_tags: None,
            placeholder: None,
            field_hints: None,
        }),
        output: Some(OutputConfig { output_type: OutputType::Audio }),
        ..Default::default()
    };
    let warnings = lint_client_section(&c, EngineMode::OneShot);
    assert!(warnings.iter().any(|w| w.rule == "input-trigger-with-accept"));
}

#[test]
fn test_lint_field_hints_no_input() {
    let mut hints = IndexMap::new();
    hints.insert(
        "x".into(),
        FieldHint { field_type: Some(FieldType::File), accept: None, placeholder: None },
    );
    let c = ClientSection {
        relay_url: None,
        gateway_path: None,
        publish: None,
        watch: None,
        input: Some(InputConfig {
            input_type: InputType::None,
            accept: None,
            asset_tags: None,
            placeholder: None,
            field_hints: Some(hints),
        }),
        output: Some(OutputConfig { output_type: OutputType::Video }),
        ..Default::default()
    };
    let warnings = lint_client_section(&c, EngineMode::OneShot);
    assert!(warnings.iter().any(|w| w.rule == "field-hints-no-input"));
}

#[test]
fn test_lint_asset_tags_text_input() {
    let c = ClientSection {
        relay_url: None,
        gateway_path: None,
        publish: None,
        watch: None,
        input: Some(InputConfig {
            input_type: InputType::Text,
            accept: None,
            asset_tags: Some(vec!["speech".into()]),
            placeholder: Some("Enter text".into()),
            field_hints: None,
        }),
        output: Some(OutputConfig { output_type: OutputType::Audio }),
        ..Default::default()
    };
    let warnings = lint_client_section(&c, EngineMode::OneShot);
    assert!(warnings.iter().any(|w| w.rule == "asset-tags-no-input"));
}

#[test]
fn test_lint_text_no_placeholder() {
    let c = ClientSection {
        relay_url: None,
        gateway_path: None,
        publish: None,
        watch: None,
        input: Some(InputConfig {
            input_type: InputType::Text,
            accept: None,
            asset_tags: None,
            placeholder: None,
            field_hints: None,
        }),
        output: Some(OutputConfig { output_type: OutputType::Audio }),
        ..Default::default()
    };
    let warnings = lint_client_section(&c, EngineMode::OneShot);
    assert!(warnings.iter().any(|w| w.rule == "text-no-placeholder"));
}

fn http_input_node() -> serde_json::Value {
    serde_json::Value::Null // represents "no params object"
}

fn node<'a>(kind: &'a str, params: Option<&'a serde_json::Value>) -> NodeInfo<'a> {
    NodeInfo { name: kind, kind, params }
}

fn named_node<'a>(
    name: &'a str,
    kind: &'a str,
    params: Option<&'a serde_json::Value>,
) -> NodeInfo<'a> {
    NodeInfo { name, kind, params }
}

// Rule 13 — input-requires-http-input
#[test]
fn test_lint_input_requires_http_input() {
    let c = oneshot_client(); // input.type = file_upload
    let nodes: Vec<NodeInfo<'_>> = vec![]; // no http_input
    let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
    assert!(warnings.iter().any(|w| w.rule == "input-requires-http-input"));
}

#[test]
fn test_lint_input_requires_http_input_clean() {
    let c = oneshot_client();
    let null = http_input_node();
    let nodes = vec![node("streamkit::http_input", Some(&null))];
    let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
    assert!(
        !warnings.iter().any(|w| w.rule == "input-requires-http-input"),
        "Should not warn when http_input exists: {warnings:?}"
    );
}

// Rule 14 — input-none-has-http-input
#[test]
fn test_lint_input_none_has_http_input() {
    let c = ClientSection {
        input: Some(InputConfig {
            input_type: InputType::None,
            accept: None,
            asset_tags: None,
            placeholder: None,
            field_hints: None,
        }),
        output: Some(OutputConfig { output_type: OutputType::Video }),
        ..Default::default()
    };
    let null = http_input_node();
    let nodes = vec![node("streamkit::http_input", Some(&null))];
    let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
    assert!(warnings.iter().any(|w| w.rule == "input-none-has-http-input"));
}

// Rule 15 — field-hint-unknown-field
#[test]
fn test_lint_field_hint_unknown_field() {
    let mut hints = IndexMap::new();
    hints.insert(
        "nonexistent".into(),
        FieldHint { field_type: Some(FieldType::Text), accept: None, placeholder: None },
    );
    let c = ClientSection {
        input: Some(InputConfig {
            input_type: InputType::FileUpload,
            accept: Some("audio/*".into()),
            asset_tags: None,
            placeholder: None,
            field_hints: Some(hints),
        }),
        output: Some(OutputConfig { output_type: OutputType::Audio }),
        ..Default::default()
    };
    // http_input with no explicit field/fields → default field is "media"
    let null = http_input_node();
    let nodes = vec![node("streamkit::http_input", Some(&null))];
    let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
    assert!(
        warnings.iter().any(|w| w.rule == "field-hint-unknown-field"),
        "Should warn for unknown field hint name: {warnings:?}"
    );
}

#[test]
fn test_lint_field_hint_known_field_clean() {
    let mut hints = IndexMap::new();
    hints.insert(
        "media".into(),
        FieldHint {
            field_type: Some(FieldType::File),
            accept: Some("audio/*".into()),
            placeholder: None,
        },
    );
    let c = ClientSection {
        input: Some(InputConfig {
            input_type: InputType::FileUpload,
            accept: Some("audio/*".into()),
            asset_tags: None,
            placeholder: None,
            field_hints: Some(hints),
        }),
        output: Some(OutputConfig { output_type: OutputType::Audio }),
        ..Default::default()
    };
    let null = http_input_node();
    let nodes = vec![node("streamkit::http_input", Some(&null))];
    let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
    assert!(
        !warnings.iter().any(|w| w.rule == "field-hint-unknown-field"),
        "Should not warn for default 'media' field: {warnings:?}"
    );
}

#[test]
fn test_lint_field_hint_explicit_fields_array() {
    let mut hints = IndexMap::new();
    hints.insert(
        "prompt".into(),
        FieldHint {
            field_type: Some(FieldType::Text),
            accept: None,
            placeholder: Some("Enter text".into()),
        },
    );
    let c = ClientSection {
        input: Some(InputConfig {
            input_type: InputType::FileUpload,
            accept: Some("audio/*".into()),
            asset_tags: None,
            placeholder: None,
            field_hints: Some(hints),
        }),
        output: Some(OutputConfig { output_type: OutputType::Audio }),
        ..Default::default()
    };
    let params = serde_json::json!({
        "fields": [
            { "name": "media" },
            { "name": "prompt" }
        ]
    });
    let nodes = vec![node("streamkit::http_input", Some(&params))];
    let warnings = lint_client_against_nodes(&c, EngineMode::OneShot, &nodes);
    assert!(
        !warnings.iter().any(|w| w.rule == "field-hint-unknown-field"),
        "Should not warn when hint matches declared field: {warnings:?}"
    );
}

// Rule 16 — publish-no-transport
#[test]
fn test_lint_publish_no_transport() {
    let c = dynamic_client();
    let nodes: Vec<NodeInfo<'_>> = vec![]; // no MoQ nodes
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(warnings.iter().any(|w| w.rule == "publish-no-transport"));
}

#[test]
fn test_lint_publish_with_peer_clean() {
    let c = dynamic_client();
    let params = serde_json::json!({
        "gateway_path": "/moq/test",
        "input_broadcasts": ["input"],
        "output_broadcast": "output"
    });
    let nodes = vec![node("transport::moq::peer", Some(&params))];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        !warnings.iter().any(|w| w.rule == "publish-no-transport"),
        "Should not warn when peer exists: {warnings:?}"
    );
}

// Rule 17 — watch-no-transport
#[test]
fn test_lint_watch_no_transport() {
    let c = ClientSection {
        gateway_path: Some("/moq/test".into()),
        watch: Some(WatchConfig {
            broadcast: Some("output".into()),
            mse_path: None,
            audio: true,
            video: true,
        }),
        ..Default::default()
    };
    let nodes: Vec<NodeInfo<'_>> = vec![]; // no MoQ nodes
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(warnings.iter().any(|w| w.rule == "watch-no-transport"));
}

// Rule 17 — watch-no-transport (MSE-only should NOT trigger)
#[test]
fn test_lint_watch_no_transport_mse_only() {
    let c = ClientSection {
        watch: Some(WatchConfig {
            broadcast: None,
            mse_path: Some("/video".into()),
            audio: false,
            video: true,
        }),
        ..Default::default()
    };
    let nodes: Vec<NodeInfo<'_>> = vec![]; // no MoQ nodes
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        !warnings.iter().any(|w| w.rule == "watch-no-transport"),
        "MSE-only watch should not trigger watch-no-transport: {warnings:?}"
    );
}

// Rule 3 — missing-gateway (MSE-only should NOT trigger)
#[test]
fn test_lint_missing_gateway_mse_only() {
    let c = ClientSection {
        watch: Some(WatchConfig {
            broadcast: None,
            mse_path: Some("/video".into()),
            audio: false,
            video: true,
        }),
        ..Default::default()
    };
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        !warnings.iter().any(|w| w.rule == "missing-gateway"),
        "MSE-only watch should not trigger missing-gateway: {warnings:?}"
    );
}

// Rule 18 — gateway-path-mismatch
#[test]
fn test_lint_gateway_path_mismatch() {
    let c = ClientSection {
        gateway_path: Some("/moq/wrong".into()),
        publish: Some(PublishConfig {
            broadcast: "input".into(),
            tracks: vec![PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            }],
        }),
        ..Default::default()
    };
    let params = serde_json::json!({
        "gateway_path": "/moq/correct",
        "input_broadcasts": ["input"]
    });
    let nodes = vec![node("transport::moq::peer", Some(&params))];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        warnings.iter().any(|w| w.rule == "gateway-path-mismatch"),
        "Should warn when gateway_path differs: {warnings:?}"
    );
}

#[test]
fn test_lint_gateway_path_match_clean() {
    let c = dynamic_client(); // gateway_path = /moq/test
    let params = serde_json::json!({
        "gateway_path": "/moq/test",
        "input_broadcasts": ["input"],
        "output_broadcast": "output"
    });
    let nodes = vec![node("transport::moq::peer", Some(&params))];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        !warnings.iter().any(|w| w.rule == "gateway-path-mismatch"),
        "Should not warn when gateway_path matches: {warnings:?}"
    );
}

// Rule 19 — relay-url-mismatch
#[test]
fn test_lint_relay_url_mismatch() {
    let c = ClientSection {
        relay_url: Some("https://relay.example.com".into()),
        publish: Some(PublishConfig {
            broadcast: "input".into(),
            tracks: vec![PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            }],
        }),
        ..Default::default()
    };
    let params = serde_json::json!({
        "url": "https://other-relay.example.com",
        "broadcast": "input"
    });
    let nodes = vec![node("transport::moq::publisher", Some(&params))];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        warnings.iter().any(|w| w.rule == "relay-url-mismatch"),
        "Should warn when relay_url differs: {warnings:?}"
    );
}

#[test]
fn test_lint_relay_url_match_clean() {
    let c = ClientSection {
        relay_url: Some("https://relay.example.com".into()),
        publish: Some(PublishConfig {
            broadcast: "input".into(),
            tracks: vec![PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            }],
        }),
        ..Default::default()
    };
    let params = serde_json::json!({
        "url": "https://relay.example.com",
        "broadcast": "input"
    });
    let nodes = vec![node("transport::moq::subscriber", Some(&params))];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        !warnings.iter().any(|w| w.rule == "relay-url-mismatch"),
        "Should not warn when relay_url matches: {warnings:?}"
    );
}

// Rule 20 — broadcast-mismatch
#[test]
fn test_lint_broadcast_mismatch_publish() {
    let c = ClientSection {
        gateway_path: Some("/moq/test".into()),
        publish: Some(PublishConfig {
            broadcast: "wrong_name".into(),
            tracks: vec![PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            }],
        }),
        ..Default::default()
    };
    let params = serde_json::json!({
        "gateway_path": "/moq/test",
        "input_broadcasts": ["camera"],
        "output_broadcast": "output"
    });
    let nodes = vec![node("transport::moq::peer", Some(&params))];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        warnings.iter().any(|w| w.rule == "broadcast-mismatch"),
        "Should warn when publish.broadcast doesn't match any node broadcast: {warnings:?}"
    );
}

#[test]
fn test_lint_broadcast_mismatch_watch() {
    let c = ClientSection {
        gateway_path: Some("/moq/test".into()),
        watch: Some(WatchConfig {
            broadcast: Some("wrong_name".into()),
            mse_path: None,
            audio: true,
            video: true,
        }),
        ..Default::default()
    };
    let params = serde_json::json!({
        "gateway_path": "/moq/test",
        "input_broadcasts": ["camera"],
        "output_broadcast": "output"
    });
    let nodes = vec![node("transport::moq::peer", Some(&params))];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        warnings.iter().any(|w| w.rule == "broadcast-mismatch"),
        "Should warn when watch.broadcast doesn't match any node broadcast: {warnings:?}"
    );
}

#[test]
fn test_lint_broadcast_match_clean() {
    let c = dynamic_client(); // publish=input, watch=output
    let params = serde_json::json!({
        "gateway_path": "/moq/test",
        "input_broadcasts": ["input"],
        "output_broadcast": "output"
    });
    let nodes = vec![node("transport::moq::peer", Some(&params))];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        !warnings.iter().any(|w| w.rule == "broadcast-mismatch"),
        "Should not warn when broadcast names match: {warnings:?}"
    );
}

// Rule 21 — control-unknown-node
#[test]
fn test_lint_control_unknown_node() {
    let c = ClientSection {
        controls: Some(vec![ControlConfig {
            label: "Show".into(),
            control_type: ControlType::Toggle,
            node: "nonexistent".into(),
            property: "properties.show".into(),
            group: None,
            default: None,
            min: None,
            max: None,
            step: None,
            value: None,
            options: None,
        }]),
        ..Default::default()
    };
    let nodes = vec![named_node("lower_third", "plugin::slint", None)];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        warnings.iter().any(|w| w.rule == "control-unknown-node"),
        "Should warn when control targets unknown node: {warnings:?}"
    );
}

#[test]
fn test_lint_control_known_node_clean() {
    let c = ClientSection {
        controls: Some(vec![ControlConfig {
            label: "Show".into(),
            control_type: ControlType::Toggle,
            node: "lower_third".into(),
            property: "properties.show".into(),
            group: None,
            default: None,
            min: None,
            max: None,
            step: None,
            value: None,
            options: None,
        }]),
        ..Default::default()
    };
    let nodes = vec![named_node("lower_third", "plugin::slint", None)];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        !warnings.iter().any(|w| w.rule == "control-unknown-node"),
        "Should not warn when control targets known node: {warnings:?}"
    );
}

// Rule 22 — control-number-no-bounds
#[test]
fn test_lint_control_number_no_bounds() {
    let c = ClientSection {
        controls: Some(vec![ControlConfig {
            label: "Score".into(),
            control_type: ControlType::Number,
            node: "scoreboard".into(),
            property: "properties.home_score".into(),
            group: None,
            default: None,
            min: None,
            max: None,
            step: None,
            value: None,
            options: None,
        }]),
        ..Default::default()
    };
    let nodes = vec![named_node("scoreboard", "plugin::slint", None)];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        warnings.iter().any(|w| w.rule == "control-number-no-bounds"),
        "Should warn when number control has no min/max: {warnings:?}"
    );
}

#[test]
fn test_lint_control_number_with_bounds_clean() {
    let c = ClientSection {
        controls: Some(vec![ControlConfig {
            label: "Score".into(),
            control_type: ControlType::Number,
            node: "scoreboard".into(),
            property: "properties.home_score".into(),
            group: None,
            default: None,
            min: Some(0.0),
            max: Some(99.0),
            step: Some(1.0),
            value: None,
            options: None,
        }]),
        ..Default::default()
    };
    let nodes = vec![named_node("scoreboard", "plugin::slint", None)];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        !warnings.iter().any(|w| w.rule == "control-number-no-bounds"),
        "Should not warn when number control has min and max: {warnings:?}"
    );
}

// Rule 23 — control-select-no-options
#[test]
fn test_lint_control_select_no_options() {
    let c = ClientSection {
        controls: Some(vec![ControlConfig {
            label: "Page".into(),
            control_type: ControlType::Select,
            node: "web_overlay".into(),
            property: "url".into(),
            group: None,
            default: None,
            min: None,
            max: None,
            step: None,
            value: None,
            options: None,
        }]),
        ..Default::default()
    };
    let nodes = vec![named_node("web_overlay", "plugin::native::servo", None)];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        warnings.iter().any(|w| w.rule == "control-select-no-options"),
        "Should warn when select control has no options: {warnings:?}"
    );
}

#[test]
fn test_lint_control_select_with_options_clean() {
    let c = ClientSection {
        controls: Some(vec![ControlConfig {
            label: "Page".into(),
            control_type: ControlType::Select,
            node: "web_overlay".into(),
            property: "url".into(),
            group: None,
            default: None,
            min: None,
            max: None,
            step: None,
            value: None,
            options: Some(vec![
                SelectOption {
                    label: "Home".into(),
                    value: serde_json::json!("https://streamkit.dev"),
                },
                SelectOption {
                    label: "Docs".into(),
                    value: serde_json::json!("https://servo.org"),
                },
            ]),
        }]),
        ..Default::default()
    };
    let nodes = vec![named_node("web_overlay", "plugin::native::servo", None)];
    let warnings = lint_client_against_nodes(&c, EngineMode::Dynamic, &nodes);
    assert!(
        !warnings.iter().any(|w| w.rule == "control-select-no-options"),
        "Should not warn when select control has options: {warnings:?}"
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_tracks_audio_only_parsed() {
    let yaml = r#"
mode: dynamic
nodes:
  peer:
    kind: transport::moq::peer
client:
  gateway_path: /moq/test
  publish:
    broadcast: input
    tracks:
      - kind: audio
        source: microphone
  watch:
    broadcast: output
    audio: true
    video: true
"#;
    let pipeline = parse_yaml(yaml).unwrap();
    let compiled = compile(pipeline).unwrap();
    let client = compiled.client.expect("client section should be present");
    let publish = client.publish.expect("publish config should be present");
    assert_eq!(publish.tracks.len(), 1);
    assert_eq!(publish.tracks[0].kind, TrackKind::Audio);
    assert_eq!(publish.tracks[0].source, CaptureSource::Microphone);
    assert!(publish.tracks[0].broadcast.is_none());
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_tracks_screen_source_parsed() {
    let yaml = r#"
mode: dynamic
nodes:
  peer:
    kind: transport::moq::peer
client:
  gateway_path: /moq/test
  publish:
    broadcast: input
    tracks:
      - kind: audio
        source: microphone
      - kind: video
        source: screen
  watch:
    broadcast: output
    audio: true
    video: true
"#;
    let pipeline = parse_yaml(yaml).unwrap();
    let compiled = compile(pipeline).unwrap();
    let client = compiled.client.expect("client section should be present");
    let publish = client.publish.expect("publish config should be present");
    assert_eq!(publish.tracks.len(), 2);
    assert_eq!(publish.tracks[1].kind, TrackKind::Video);
    assert_eq!(publish.tracks[1].source, CaptureSource::Screen);
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_tracks_multi_broadcast_parsed() {
    let yaml = r#"
mode: dynamic
nodes:
  peer:
    kind: transport::moq::peer
client:
  gateway_path: /moq/test
  publish:
    broadcast: screen-input
    tracks:
      - kind: audio
        source: microphone
      - kind: video
        source: screen
      - kind: video
        source: camera
        broadcast: cam-input
  watch:
    broadcast: output
    audio: true
    video: true
"#;
    let pipeline = parse_yaml(yaml).unwrap();
    let compiled = compile(pipeline).unwrap();
    let client = compiled.client.expect("client section should be present");
    let publish = client.publish.expect("publish config should be present");
    assert_eq!(publish.tracks.len(), 3);
    assert_eq!(publish.tracks[0].source, CaptureSource::Microphone);
    assert_eq!(publish.tracks[1].source, CaptureSource::Screen);
    assert_eq!(publish.tracks[2].source, CaptureSource::Camera);
    assert_eq!(publish.tracks[2].broadcast.as_deref(), Some("cam-input"));
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_tracks_roundtrip() {
    // Verify serde round-trip: serialize → deserialize preserves the value.
    let config = PublishConfig {
        broadcast: "test".into(),
        tracks: vec![
            PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
            PublishTrackConfig {
                kind: TrackKind::Video,
                source: CaptureSource::Screen,
                broadcast: Some("screen-input".into()),
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
        ],
    };
    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("\"source\":\"screen\""));
    assert!(json.contains("\"screen-input\""));

    let deserialized: PublishConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.tracks.len(), 2);
    assert_eq!(deserialized.tracks[1].source, CaptureSource::Screen);
    assert_eq!(deserialized.tracks[1].broadcast.as_deref(), Some("screen-input"));
}

#[test]
fn test_lint_track_kind_source_mismatch() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Audio,
            source: CaptureSource::Screen,
            broadcast: None,
            width: None,
            height: None,
            codec: None,
            max_bitrate: None,
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        warnings.iter().any(|w| w.rule == "kind-source-mismatch"),
        "Should warn when audio track uses screen source: {warnings:?}"
    );
}

#[test]
fn test_lint_track_kind_source_valid_clean() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![
            PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
            PublishTrackConfig {
                kind: TrackKind::Video,
                source: CaptureSource::Screen,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
        ],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        !warnings.iter().any(|w| w.rule == "kind-source-mismatch"),
        "Should not warn for valid kind/source combinations: {warnings:?}"
    );
}

#[test]
fn test_lint_duplicate_source() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![
            PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
            PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
        ],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        warnings.iter().any(|w| w.rule == "duplicate-source"),
        "Should warn when same source appears twice in same broadcast: {warnings:?}"
    );
}

#[test]
fn test_lint_duplicate_source_different_broadcast_clean() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![
            PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
            PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: Some("other-input".into()),
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
        ],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        !warnings.iter().any(|w| w.rule == "duplicate-source"),
        "Should not warn when same source is in different broadcasts: {warnings:?}"
    );
}

/// Regression: duplicate-source lint used to check CaptureSource alone,
/// causing a false positive when different track kinds shared the same
/// source (e.g. audio+microphone and video+microphone).  The latter is
/// already caught by kind-source-mismatch, so duplicate-source should
/// only fire on identical (kind, source) pairs.
#[test]
fn test_lint_duplicate_source_different_kind_same_source_no_false_positive() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![
            PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
            PublishTrackConfig {
                kind: TrackKind::Video,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: None,
                max_bitrate: None,
            },
        ],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        !warnings.iter().any(|w| w.rule == "duplicate-source"),
        "Should not warn when different kinds share the same source: {warnings:?}"
    );
    // The (video, microphone) track should trigger kind-source-mismatch instead
    assert!(
        warnings.iter().any(|w| w.rule == "kind-source-mismatch"),
        "Should warn about kind-source mismatch for (video, microphone): {warnings:?}"
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn test_tracks_media_hints_parsed() {
    let yaml = r#"
mode: dynamic
nodes:
  peer:
    kind: transport::moq::peer
client:
  gateway_path: /moq/test
  publish:
    broadcast: input
    tracks:
      - kind: video
        source: screen
        width: 1280
        height: 720
        codec: vp9
        max_bitrate: 2500
      - kind: audio
        source: microphone
        codec: opus
        max_bitrate: 32
  watch:
    broadcast: output
    audio: true
    video: true
"#;
    let pipeline = parse_yaml(yaml).unwrap();
    let compiled = compile(pipeline).unwrap();
    let client = compiled.client.expect("client section should be present");
    let publish = client.publish.expect("publish config should be present");
    assert_eq!(publish.tracks.len(), 2);

    let video = &publish.tracks[0];
    assert_eq!(video.kind, TrackKind::Video);
    assert_eq!(video.width, Some(1280));
    assert_eq!(video.height, Some(720));
    assert_eq!(video.codec.as_deref(), Some("vp9"));
    assert_eq!(video.max_bitrate, Some(2500));

    let audio = &publish.tracks[1];
    assert_eq!(audio.kind, TrackKind::Audio);
    assert!(audio.width.is_none());
    assert!(audio.height.is_none());
    assert_eq!(audio.codec.as_deref(), Some("opus"));
    assert_eq!(audio.max_bitrate, Some(32));
}

#[test]
fn test_lint_dimensions_on_audio_track() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Audio,
            source: CaptureSource::Microphone,
            broadcast: None,
            width: Some(1280),
            height: None,
            codec: None,
            max_bitrate: None,
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        warnings.iter().any(|w| w.rule == "dimensions-on-audio"),
        "Should warn when audio track sets width/height: {warnings:?}"
    );
}

#[test]
fn test_lint_partial_dimensions() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Video,
            source: CaptureSource::Camera,
            broadcast: None,
            width: Some(1280),
            height: None,
            codec: None,
            max_bitrate: None,
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        warnings.iter().any(|w| w.rule == "partial-dimensions"),
        "Should warn when video track sets width without height: {warnings:?}"
    );
}

#[test]
fn test_lint_partial_dimensions_clean() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Video,
            source: CaptureSource::Screen,
            broadcast: None,
            width: Some(1280),
            height: Some(720),
            codec: None,
            max_bitrate: None,
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        !warnings.iter().any(|w| w.rule == "partial-dimensions"),
        "Should not warn when both width and height are set: {warnings:?}"
    );
}

#[test]
fn test_lint_unrecognized_video_codec() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Video,
            source: CaptureSource::Camera,
            broadcast: None,
            width: None,
            height: None,
            codec: Some("hevc".into()),
            max_bitrate: None,
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        warnings.iter().any(|w| w.rule == "unrecognized-codec"),
        "Should warn for unrecognized video codec: {warnings:?}"
    );
}

#[test]
fn test_lint_unrecognized_audio_codec() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Audio,
            source: CaptureSource::Microphone,
            broadcast: None,
            width: None,
            height: None,
            codec: Some("mp3".into()),
            max_bitrate: None,
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        warnings.iter().any(|w| w.rule == "unrecognized-codec"),
        "Should warn for unrecognized audio codec: {warnings:?}"
    );
}

#[test]
fn test_lint_recognized_codecs_clean() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![
            PublishTrackConfig {
                kind: TrackKind::Video,
                source: CaptureSource::Camera,
                broadcast: None,
                width: None,
                height: None,
                codec: Some("vp9".into()),
                max_bitrate: None,
            },
            PublishTrackConfig {
                kind: TrackKind::Video,
                source: CaptureSource::Screen,
                broadcast: None,
                width: None,
                height: None,
                codec: Some("av1".into()),
                max_bitrate: None,
            },
            PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: Some("opus".into()),
                max_bitrate: None,
            },
            PublishTrackConfig {
                kind: TrackKind::Video,
                source: CaptureSource::Camera,
                broadcast: None,
                width: None,
                height: None,
                codec: Some("h264".into()),
                max_bitrate: None,
            },
            PublishTrackConfig {
                kind: TrackKind::Audio,
                source: CaptureSource::Microphone,
                broadcast: None,
                width: None,
                height: None,
                codec: Some("aac".into()),
                max_bitrate: None,
            },
        ],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        !warnings.iter().any(|w| w.rule == "unrecognized-codec"),
        "Should not warn for recognized codecs: {warnings:?}"
    );
}

#[test]
fn test_lint_zero_dimension() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Video,
            source: CaptureSource::Camera,
            broadcast: None,
            width: Some(0),
            height: Some(720),
            codec: None,
            max_bitrate: None,
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        warnings.iter().any(|w| w.rule == "zero-dimension"),
        "Should warn for zero-value width: {warnings:?}"
    );
}

#[test]
fn test_lint_zero_dimension_skipped_for_audio() {
    // Audio tracks with width: 0 should fire `dimensions-on-audio` but
    // NOT `zero-dimension` — the latter is video-only.
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Audio,
            source: CaptureSource::Microphone,
            broadcast: None,
            width: Some(0),
            height: Some(720),
            codec: None,
            max_bitrate: None,
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        warnings.iter().any(|w| w.rule == "dimensions-on-audio"),
        "Should warn for dimensions on audio: {warnings:?}"
    );
    assert!(
        !warnings.iter().any(|w| w.rule == "zero-dimension"),
        "Should NOT fire zero-dimension for audio tracks: {warnings:?}"
    );
}

#[test]
fn test_lint_zero_bitrate() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Video,
            source: CaptureSource::Camera,
            broadcast: None,
            width: None,
            height: None,
            codec: None,
            max_bitrate: Some(0),
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        warnings.iter().any(|w| w.rule == "zero-bitrate"),
        "Should warn for zero-value max_bitrate: {warnings:?}"
    );
}

#[test]
fn test_lint_bitrate_on_audio() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Audio,
            source: CaptureSource::Microphone,
            broadcast: None,
            width: None,
            height: None,
            codec: None,
            max_bitrate: Some(32),
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        warnings.iter().any(|w| w.rule == "bitrate-on-audio"),
        "Should warn for max_bitrate on audio track: {warnings:?}"
    );
}

#[test]
fn test_lint_no_bitrate_on_audio_when_absent() {
    let mut c = dynamic_client();
    c.publish = Some(PublishConfig {
        broadcast: "input".into(),
        tracks: vec![PublishTrackConfig {
            kind: TrackKind::Audio,
            source: CaptureSource::Microphone,
            broadcast: None,
            width: None,
            height: None,
            codec: None,
            max_bitrate: None,
        }],
    });
    let warnings = lint_client_section(&c, EngineMode::Dynamic);
    assert!(
        !warnings.iter().any(|w| w.rule == "bitrate-on-audio"),
        "Should not warn when audio track has no max_bitrate: {warnings:?}"
    );
}

#[test]
fn test_malformed_yaml_syntax_error() {
    let yaml = "nodes:\n  foo:\n    kind: test\n  - invalid: indentation\n";
    let result = parse_yaml(yaml);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("Invalid YAML"),
        "Malformed YAML should produce a clear 'Invalid YAML' error: {err}"
    );
}

#[test]
fn test_empty_pipeline_no_nodes() {
    let yaml = "mode: dynamic\nnodes: {}\n";
    let user_pipeline = parse_yaml(yaml).unwrap();
    let pipeline = compile(user_pipeline).unwrap();
    assert!(pipeline.nodes.is_empty());
    assert!(pipeline.connections.is_empty());
}

#[test]
fn test_connection_referencing_nonexistent_node() {
    let yaml = r"
mode: dynamic
nodes:
  real_node:
    kind: test_source
    needs: ghost_node
";
    let user_pipeline = parse_yaml(yaml).unwrap();
    let result = compile(user_pipeline);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("ghost_node"), "Error should mention the non-existent node: {err}");
    assert!(err.contains("non-existent"), "Error should say 'non-existent': {err}");
}

#[test]
fn test_duplicate_node_ids_rejected() {
    let yaml = r"
mode: dynamic
nodes:
  dup:
    kind: first_kind
  dup:
    kind: second_kind
";
    let result = parse_yaml(yaml);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("duplicate"),
        "Duplicate node IDs should produce an error mentioning 'duplicate': {err}"
    );
}
