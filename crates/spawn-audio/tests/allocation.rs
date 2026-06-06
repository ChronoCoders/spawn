//! Steady-state allocation guard: with active voices and no new commands,
//! [`AudioEngine::update`] must perform zero heap allocations.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct CountingAlloc;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static COUNTING: AtomicBool = AtomicBool::new(false);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

use spawn_asset::{AssetServer, AssetServerConfig};
use spawn_audio::{AudioConfig, AudioEngine, AudioSource, PlaybackParams, Spatial};
use spawn_core::Vec3;

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
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&sample_rate.to_le_bytes());
    v.extend_from_slice(&byte_rate.to_le_bytes());
    v.extend_from_slice(&block_align.to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for _ in 0..frames {
        v.extend_from_slice(&0i16.to_le_bytes());
    }
    v
}

#[test]
fn update_is_allocation_free_in_steady_state() {
    let dir = std::env::temp_dir().join(format!("spawn_audio_alloc_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("beep.wav"), tiny_wav()).unwrap();

    let mut server = AssetServer::new(AssetServerConfig {
        root: dir.clone(),
        hot_reload: false,
        ..Default::default()
    })
    .unwrap();
    spawn_audio::register(&mut server).unwrap();
    let handle = server.load::<AudioSource>("beep.wav");
    for _ in 0..1000 {
        server.apply_loaded();
        if server.get(&handle).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(server.get(&handle).is_some());

    let mut engine = AudioEngine::new(AudioConfig {
        max_voices: 16,
        ..Default::default()
    })
    .unwrap();

    // Start several spatial voices, then flush their Start commands.
    for i in 0..8 {
        let params = PlaybackParams {
            looping: true,
            spatial: Some(Spatial {
                position: Vec3::new(i as f32, 0.0, -1.0),
                attenuation: spawn_audio::Attenuation::default(),
            }),
            ..Default::default()
        };
        engine.play(&handle, params, &server).unwrap();
    }
    // Drain the start queue and reach steady state.
    for _ in 0..4 {
        engine.update(0.016).unwrap();
    }

    // Now measure: pure update over active spatial voices, no new commands.
    COUNTING.store(true, Ordering::Relaxed);
    let before = ALLOCS.load(Ordering::Relaxed);
    for _ in 0..600 {
        engine.update(0.016).unwrap();
    }
    let after = ALLOCS.load(Ordering::Relaxed);
    COUNTING.store(false, Ordering::Relaxed);

    assert_eq!(
        after - before,
        0,
        "update allocated {} times in steady state",
        after - before
    );

    let _ = std::fs::remove_dir_all(&dir);
}
