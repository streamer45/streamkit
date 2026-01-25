use pocket_tts::TTSModel;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let model = TTSModel::load("b6369a24")?;
    let hf_path = "hf://kyutai/pocket-tts-without-voice-cloning/embeddings/cosette.safetensors";
    let local_path = pocket_tts::weights::download_if_necessary(hf_path)?;
    let voice_state = model.get_voice_state_from_prompt_file(&local_path)?;

    let text_short = "Short text example.";
    let text_medium = "This is a medium length text example that has more words but is still relatively short for a transformer model to handle without much sweat.";
    let text_long = "Alice was beginning to get very tired of sitting by her sister on the bank, and of having nothing to do: once or twice she had peeped into the book her sister was reading, but it had no pictures or conversations in it, 'and what is the use of a book,' thought Alice 'without pictures or conversations?' So she was considering in her own mind (as well as she could, for the hot day made her feel very sleepy and stupid), whether the pleasure of making a daisy-chain would be worth the trouble of getting up and picking the daisies, when suddenly a White Rabbit with pink eyes ran close by her.";

    for text in &[text_short, text_medium, text_long] {
        let start = Instant::now();
        let mut count = 0;
        for chunk in model.generate_stream(text, &voice_state) {
            let _ = chunk?;
            count += 1;
        }
        let duration = start.elapsed();
        println!(
            "Text length: {}, Frames: {}, Time: {:?}, ms/frame: {:.2}",
            text.len(),
            count,
            duration,
            duration.as_millis() as f64 / count as f64
        );
    }

    Ok(())
}
