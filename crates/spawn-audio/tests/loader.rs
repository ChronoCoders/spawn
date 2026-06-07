//! AudioLoader decode tests: a bundled WAV decodes with sane metadata,
//! unsupported extensions and garbage bytes fail explicitly.

use spawn_asset::{AssetId, AssetLoader, LoadContext};
use spawn_audio::AudioLoader;

fn ctx<'a>(path: &'a str, ext: &'a str) -> LoadContext<'a> {
    LoadContext {
        id: AssetId::from_raw(1),
        canonical_path: path,
        extension: ext,
    }
}

fn wav_with_channels(channels: u16) -> Vec<u8> {
    let sample_rate: u32 = 8000;
    let bits: u16 = 16;
    let frames: u32 = 16;
    let data_len = frames * u32::from(channels) * u32::from(bits / 8);
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits / 8);
    let block_align = channels * (bits / 8);

    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&sample_rate.to_le_bytes());
    v.extend_from_slice(&byte_rate.to_le_bytes());
    v.extend_from_slice(&block_align.to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames * u32::from(channels) {
        let s = (i as i16).wrapping_mul(1000);
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

fn tiny_wav() -> Vec<u8> {
    wav_with_channels(1)
}

#[test]
fn decodes_wav_metadata() {
    let source = AudioLoader
        .load(&tiny_wav(), &ctx("beep.wav", "wav"))
        .expect("wav decodes");
    assert_eq!(source.sample_rate(), 8000);
    // tiny_wav is mono; channels() must report the real source count.
    assert_eq!(source.channels(), 1);
    // 16 frames at 8 kHz = 2 ms.
    assert!(source.duration() > 0.0 && source.duration() < 0.1);
}

#[test]
fn decodes_stereo_wav_channels() {
    let source = AudioLoader
        .load(&wav_with_channels(2), &ctx("stereo.wav", "wav"))
        .expect("stereo wav decodes");
    assert_eq!(source.channels(), 2);
}

#[test]
fn unsupported_extension_fails() {
    let Err(err) = AudioLoader.load(&tiny_wav(), &ctx("x.mp3", "mp3")) else {
        panic!("mp3 is unsupported and must not decode");
    };
    assert!(
        err.to_string().contains("wav and ogg"),
        "error should surface UnsupportedFormat detail, got: {err}"
    );
}

#[test]
fn garbage_bytes_fail() {
    assert!(AudioLoader
        .load(b"not audio at all", &ctx("x.wav", "wav"))
        .is_err());
    // Truncated header.
    assert!(AudioLoader
        .load(&tiny_wav()[..8], &ctx("x.wav", "wav"))
        .is_err());
}
