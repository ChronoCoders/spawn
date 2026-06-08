//! On-target GPU probe: enumerates every wgpu adapter across all backends and
//! confirms a device can be created on each (DX12 included). No window/surface
//! required, so it runs in non-interactive sessions. Hardware validation only;
//! not part of the engine API.

fn main() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapters = instance.enumerate_adapters(wgpu::Backends::all());
    println!("enumerated {} adapter(s):", adapters.len());
    for a in &adapters {
        let i = a.get_info();
        let device_ok = pollster::block_on(a.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("gpuinfo-probe"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .is_ok();
        println!(
            "  name={} | backend={:?} | type={:?} | device_create={} | driver={} {}",
            i.name, i.backend, i.device_type, device_ok, i.driver, i.driver_info
        );
    }
}
