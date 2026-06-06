//! NullBackend lifecycle and state-machine tests. These run on the headless
//! host (no audio device), exercising the documented silent fallback.

use spawn_audio::{
    AudioConfig, AudioEngine, BackendKind, BusId, BusSpec, PlaybackParams, VoiceState,
};

fn null_engine() -> AudioEngine {
    let engine = AudioEngine::new(AudioConfig {
        max_voices: 4,
        buses: vec![BusSpec {
            id: BusId("sfx"),
            initial_volume: 0.8,
        }],
        ..Default::default()
    })
    .expect("engine constructs");
    assert_eq!(
        engine.backend_kind(),
        BackendKind::Null,
        "headless host must fall back to NullBackend"
    );
    engine
}

#[test]
fn unloaded_handle_play_errors() {
    let dir = std::env::temp_dir().join(format!("spawn_audio_unloaded_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut server = spawn_asset::AssetServer::new(spawn_asset::AssetServerConfig {
        root: dir.clone(),
        hot_reload: false,
        ..Default::default()
    })
    .unwrap();
    spawn_audio::register(&mut server).unwrap();

    // The file does not exist, so the handle never reaches Loaded.
    let handle = server.load::<spawn_audio::AudioSource>("missing.wav");
    for _ in 0..100 {
        server.apply_loaded();
        if server.get(&handle).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(server.get(&handle).is_none(), "missing file must not load");

    let mut engine = null_engine();
    let err = engine
        .play(&handle, PlaybackParams::default(), &server)
        .expect_err("unloaded handle must not play");
    assert!(matches!(err, spawn_audio::AudioError::AssetNotLoaded));
    assert_eq!(engine.active_voice_count(), 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn suspend_resume_idempotent_over_frames() {
    let mut engine = null_engine();
    for _ in 0..120 {
        engine.update(1.0 / 60.0).unwrap();
    }
    engine.suspend().unwrap();
    engine.suspend().unwrap();
    engine.resume().unwrap();
    engine.resume().unwrap();
    assert_eq!(engine.active_voice_count(), 0);
}

#[test]
fn bus_volume_unknown_and_master() {
    let mut engine = null_engine();
    assert!(engine.bus_volume(BusId("nope")).is_err());
    assert!(engine.set_bus_volume(BusId("nope"), 0.5).is_err());
    engine.set_bus_volume(BusId::MASTER, 0.3).unwrap();
    let _ = PlaybackParams::default();
}

#[test]
fn lifecycle_transitions_via_server() {
    // Build a real AssetServer over a temp dir holding a tiny WAV, load it,
    // pump, then drive the voice through its state machine.
    let dir = std::env::temp_dir().join(format!("spawn_audio_null_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("beep.wav");
    std::fs::write(&wav, tiny_wav()).unwrap();

    let mut server = spawn_asset::AssetServer::new(spawn_asset::AssetServerConfig {
        root: dir.clone(),
        hot_reload: false,
        ..Default::default()
    })
    .unwrap();
    spawn_audio::register(&mut server).unwrap();

    let handle = server.load::<spawn_audio::AudioSource>("beep.wav");
    // Pump the IO pool until the asset is loaded.
    for _ in 0..1000 {
        server.apply_loaded();
        if server.get(&handle).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(server.get(&handle).is_some(), "wav should load");

    let mut engine = null_engine();
    let h = engine
        .play(&handle, PlaybackParams::default(), &server)
        .expect("play enqueues");
    assert_eq!(engine.voice_state(h).unwrap(), VoiceState::Playing);
    assert_eq!(engine.active_voice_count(), 1);

    engine.update(0.016).unwrap();
    assert_eq!(engine.voice_state(h).unwrap(), VoiceState::Playing);

    engine.pause(h).unwrap();
    assert_eq!(engine.voice_state(h).unwrap(), VoiceState::Paused);
    engine.update(0.016).unwrap();

    engine.resume_handle(h).unwrap();
    assert_eq!(engine.voice_state(h).unwrap(), VoiceState::Playing);
    engine.update(0.016).unwrap();

    engine.set_volume(h, 0.5).unwrap();
    engine.set_pitch(h, 1.5).unwrap();
    engine.update(0.016).unwrap();

    engine.stop(h).unwrap();
    engine.update(0.016).unwrap();
    // After stop is applied the slot is reaped; the handle is now stale.
    assert!(engine.voice_state(h).is_err());
    assert_eq!(engine.active_voice_count(), 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn voice_limit_enforced() {
    let dir = std::env::temp_dir().join(format!("spawn_audio_limit_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("beep.wav"), tiny_wav()).unwrap();

    let mut server = spawn_asset::AssetServer::new(spawn_asset::AssetServerConfig {
        root: dir.clone(),
        hot_reload: false,
        ..Default::default()
    })
    .unwrap();
    spawn_audio::register(&mut server).unwrap();
    let handle = server.load::<spawn_audio::AudioSource>("beep.wav");
    for _ in 0..1000 {
        server.apply_loaded();
        if server.get(&handle).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    let mut engine = AudioEngine::new(AudioConfig {
        max_voices: 2,
        ..Default::default()
    })
    .unwrap();

    assert!(engine
        .play(&handle, PlaybackParams::default(), &server)
        .is_ok());
    assert!(engine
        .play(&handle, PlaybackParams::default(), &server)
        .is_ok());
    let third = engine.play(&handle, PlaybackParams::default(), &server);
    assert!(matches!(third, Err(spawn_audio::AudioError::VoiceLimit)));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A minimal valid 16-bit mono PCM WAV: 8 frames at 8 kHz.
fn tiny_wav() -> Vec<u8> {
    let sample_rate: u32 = 8000;
    let channels: u16 = 1;
    let bits: u16 = 16;
    let frames: u32 = 8;
    let data_len = frames * u32::from(channels) * u32::from(bits / 8);
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits / 8);
    let block_align = channels * (bits / 8);

    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&sample_rate.to_le_bytes());
    v.extend_from_slice(&byte_rate.to_le_bytes());
    v.extend_from_slice(&block_align.to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let s = ((i as i32 * 4000) - 14000) as i16;
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}
