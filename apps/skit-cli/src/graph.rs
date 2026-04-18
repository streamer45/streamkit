// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::fmt::Write;

use streamkit_api::{Connection, Pipeline};

/// Supported output formats for graph visualization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum GraphFormat {
    /// Human-readable text table (default).
    #[default]
    Text,
    /// Graphviz DOT language.
    Dot,
    /// Mermaid diagram syntax.
    Mermaid,
}

/// Render a compiled [`Pipeline`] in the requested format.
pub fn render_graph(pipeline: &Pipeline, format: GraphFormat) -> String {
    match format {
        GraphFormat::Text => render_text(pipeline),
        GraphFormat::Dot => render_dot(pipeline),
        GraphFormat::Mermaid => render_mermaid(pipeline),
    }
}

fn render_text(pipeline: &Pipeline) -> String {
    let mut out = String::new();

    if let Some(name) = &pipeline.name {
        let _ = writeln!(out, "Pipeline: {name}");
    }
    if let Some(desc) = &pipeline.description {
        let _ = writeln!(out, "  {desc}");
    }
    if pipeline.name.is_some() || pipeline.description.is_some() {
        out.push('\n');
    }

    out.push_str("Nodes:\n");
    for (id, node) in &pipeline.nodes {
        let _ = writeln!(out, "  {id}  ({})", node.kind);
    }

    out.push('\n');
    out.push_str("Connections:\n");
    if pipeline.connections.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for conn in &pipeline.connections {
            let _ = writeln!(
                out,
                "  {}:{} -> {}:{}",
                conn.from_node, conn.from_pin, conn.to_node, conn.to_pin
            );
        }
    }

    out
}

fn render_dot(pipeline: &Pipeline) -> String {
    let mut out = String::new();
    out.push_str("digraph {\n");

    for (id, node) in &pipeline.nodes {
        let _ = writeln!(out, "  \"{id}\" [label=\"{id}\\n{}\"];", node.kind);
    }

    for Connection { from_node, from_pin, to_node, to_pin, .. } in &pipeline.connections {
        let _ =
            writeln!(out, "  \"{from_node}\" -> \"{to_node}\" [label=\"{from_pin} -> {to_pin}\"];");
    }

    out.push_str("}\n");
    out
}

fn render_mermaid(pipeline: &Pipeline) -> String {
    let mut out = String::new();
    out.push_str("graph LR\n");

    for (id, node) in &pipeline.nodes {
        let _ = writeln!(out, "  {id}[\"{id}<br>{}\"]", node.kind);
    }

    for Connection { from_node, from_pin, to_node, to_pin, .. } in &pipeline.connections {
        let _ = writeln!(out, "  {from_node} -->|{from_pin} -> {to_pin}| {to_node}");
    }

    out
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_pipeline() -> Pipeline {
        let yaml = r"
name: test-pipeline
description: A test pipeline
nodes:
  src:
    kind: audio::file_reader
    params:
      path: test.wav
  enc:
    kind: audio::opus::encoder
    needs: src
  out:
    kind: transport::whip_output
    needs: enc
";
        let user = streamkit_api::yaml::parse_yaml(yaml).expect("parse sample yaml");
        streamkit_api::yaml::compile(user).expect("compile sample yaml")
    }

    #[test]
    fn text_output_contains_nodes_and_connections() {
        let pipeline = sample_pipeline();
        let text = render_text(&pipeline);

        assert!(text.contains("Pipeline: test-pipeline"));
        assert!(text.contains("src  (audio::file_reader)"));
        assert!(text.contains("enc  (audio::opus::encoder)"));
        assert!(text.contains("out  (transport::whip_output)"));
        assert!(text.contains("src:out -> enc:in"));
        assert!(text.contains("enc:out -> out:in"));
    }

    #[test]
    fn dot_output_is_valid_graphviz() {
        let pipeline = sample_pipeline();
        let dot = render_dot(&pipeline);

        assert!(dot.starts_with("digraph {"));
        assert!(dot.contains("\"src\" [label=\"src\\naudio::file_reader\"]"));
        assert!(dot.contains("\"src\" -> \"enc\""));
        assert!(dot.ends_with("}\n"));
    }

    #[test]
    fn mermaid_output_is_valid() {
        let pipeline = sample_pipeline();
        let mermaid = render_mermaid(&pipeline);

        assert!(mermaid.starts_with("graph LR"));
        assert!(mermaid.contains("src[\"src<br>audio::file_reader\"]"));
        assert!(mermaid.contains("src -->|out -> in| enc"));
    }

    #[test]
    fn empty_connections_shows_none() {
        let mut pipeline = sample_pipeline();
        pipeline.connections.clear();
        let text = render_text(&pipeline);

        assert!(text.contains("(none)"));
    }
}
