// SPDX-FileCopyrightText: Copyright (c) 2024 Pocket TTS Contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

fn main() {
    let cursor = std::io::Cursor::new(vec![]);
    let reader = hound::WavReader::new(cursor);
    match reader {
        Ok(_) => println!("Unexpected success"),
        Err(e) => println!("Error Variant Debug: {:?}", e),
    }
}
