//! On-target device test (workflow step 5). Ignored by default; run manually on
//! hardware with a real audio device:
//!   cargo test -p spawn-audio --test device -- --ignored --nocapture

use spawn_asset::{AssetServer, AssetServerConfig};
use spawn_audio::{
    Attenuation, AudioConfig, AudioEngine, AudioSource, BackendKind, PlaybackParams, Spatial,
};
use spawn_core::Vec3;

#[test]
#[ignore = "requires a real audio device; run on target hardware"]
fn plays_spatial_clip_on_device() {
    let root = std::path::PathBuf::from(
        std::env::var("SPAWN_AUDIO_FIXTURES").unwrap_or_else(|_| "assets".to_string()),
    );
    let mut server = AssetServer::new(AssetServerConfig {
        root,
        hot_reload: false,
        ..Default::default()
    })
    .expect("asset server");
    spawn_audio::register(&mut server).expect("register loader");

    let handle = server.load::<AudioSource>("loop.ogg");
    for _ in 0..2000 {
        server.apply_loaded();
        if server.get(&handle).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(server.get(&handle).is_some(), "loop.ogg must load");

    let mut engine = AudioEngine::new(AudioConfig::default()).expect("engine");
    assert_eq!(
        engine.backend_kind(),
        BackendKind::Device,
        "on-target run must open a real device"
    );

    let params = PlaybackParams {
        looping: true,
        spatial: Some(Spatial {
            position: Vec3::new(-10.0, 0.0, 0.0),
            attenuation: Attenuation::default(),
        }),
        ..Default::default()
    };
    let h = engine.play(&handle, params, &server).expect("play");

    for step in 0..=100 {
        let x = -10.0 + (step as f32) * 0.2;
        engine.set_position(h, Vec3::new(x, 0.0, 0.0)).unwrap();
        engine.update(0.016).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }

    engine.suspend().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    engine.resume().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));

    engine.stop(h).unwrap();
    engine.update(0.016).unwrap();
}
