//! Breadcrumbs around the GPU calls that reach the graphics driver: every shader
//! module and pipeline the game creates, and the instance, adapter, device and
//! surface steps. A driver that crashes or hangs gives us no error to report, so
//! the last line in the session log, the crash report and the watchdog have to
//! say what it was doing. Opinionated host behavior, requested by John on
//! 2026-11-02; docs/spec/startup-diagnostics.md. The threshold and the line budget
//! are agent choices.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// A driver call this slow gets a timing line of its own, since a slow compile is
/// evidence on its own. Fitted: a normal pipeline takes milliseconds on Vulkan.
pub(crate) const SLOW_STEP: Duration = Duration::from_secs(1);
/// At most this many step lines a session, at about 100 bytes each. A launch
/// writes about 65 (measured on Linux, 2026-11-02); each anti-aliasing change or
/// theater load repeats part of that. The budget keeps a long session from spending the 5 MiB routine log
/// on them. The crash report and watchdog keep working past it.
const MAX_STEP_LINES: usize = 2000;

static STEP_LINES: AtomicUsize = AtomicUsize::new(0);

/// One driver-reaching call in progress. Recorded before the call, whatever
/// happens to it; dropping it records that the call returned.
pub(crate) struct Step<'a> {
    what: &'static str,
    label: &'a str,
    started: Instant,
}

/// Names the next driver call in the log, already on disk when this returns.
pub(crate) fn begin<'a>(what: &'static str, label: &'a str) -> Step<'a> {
    tore_diagnostics_native::crash_context::GPU_STEP.set_parts(&["in ", what, " '", label, "'"]);
    crate::diagnostics::gpu_step_started();
    let line = STEP_LINES.fetch_add(1, Ordering::Relaxed);
    if line < MAX_STEP_LINES {
        crate::diagnostics::breadcrumb(&format!("GPU setup: {what} '{label}'"));
    } else if line == MAX_STEP_LINES {
        crate::diagnostics::breadcrumb(
            "GPU setup: step line budget reached; crash reports still name the last step",
        );
    }
    Step {
        what,
        label,
        started: Instant::now(),
    }
}

impl Drop for Step<'_> {
    fn drop(&mut self) {
        tore_diagnostics_native::crash_context::GPU_STEP.set_parts(&[
            "after ",
            self.what,
            " '",
            self.label,
            "' returned",
        ]);
        crate::diagnostics::gpu_step_finished();
        let elapsed = self.started.elapsed();
        if elapsed >= SLOW_STEP {
            log::info!(
                "GPU setup: {} '{}' took {:.1} s",
                self.what,
                self.label,
                elapsed.as_secs_f64()
            );
        }
    }
}

fn named(label: Option<&str>) -> &str {
    label.unwrap_or("unlabeled")
}

/// The wgpu creation calls with their step recorded first. Clippy rejects the
/// plain ones outside tests (`clippy.toml`).
pub(crate) trait NamedSteps {
    fn named_shader_module(
        &self,
        descriptor: wgpu::ShaderModuleDescriptor<'_>,
    ) -> wgpu::ShaderModule;
    fn named_render_pipeline(
        &self,
        descriptor: &wgpu::RenderPipelineDescriptor<'_>,
    ) -> wgpu::RenderPipeline;
    fn named_compute_pipeline(
        &self,
        descriptor: &wgpu::ComputePipelineDescriptor<'_>,
    ) -> wgpu::ComputePipeline;
}

#[allow(clippy::disallowed_methods)]
impl NamedSteps for wgpu::Device {
    fn named_shader_module(
        &self,
        descriptor: wgpu::ShaderModuleDescriptor<'_>,
    ) -> wgpu::ShaderModule {
        let label = descriptor.label.map(str::to_owned);
        let _step = begin("shader module", named(label.as_deref()));
        self.create_shader_module(descriptor)
    }
    fn named_render_pipeline(
        &self,
        descriptor: &wgpu::RenderPipelineDescriptor<'_>,
    ) -> wgpu::RenderPipeline {
        let _step = begin("render pipeline", named(descriptor.label));
        self.create_render_pipeline(descriptor)
    }
    fn named_compute_pipeline(
        &self,
        descriptor: &wgpu::ComputePipelineDescriptor<'_>,
    ) -> wgpu::ComputePipeline {
        let _step = begin("compute pipeline", named(descriptor.label));
        self.create_compute_pipeline(descriptor)
    }
}

/// The adapter facts in one line, for the log and the crash report, before the
/// device request: that call can be the one that dies.
pub(crate) fn note_adapter(info: &wgpu::AdapterInfo) {
    let line = format!(
        "adapter='{}' vendor=0x{:04x} device=0x{:04x} backend={:?} type={:?} driver='{}' driver_info='{}'",
        info.name,
        info.vendor,
        info.device,
        info.backend,
        info.device_type,
        info.driver,
        info.driver_info
    );
    tore_diagnostics_native::crash_context::ADAPTER.set(&line);
    crate::diagnostics::breadcrumb(&format!("GPU setup: selected {line}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_name_what_they_wrap_and_whether_it_returned() {
        let mut out = [0; tore_diagnostics_native::crash_context::CELL_BYTES];
        let read = |out: &mut [u8; tore_diagnostics_native::crash_context::CELL_BYTES]| {
            tore_diagnostics_native::crash_context::GPU_STEP
                .read(out)
                .to_owned()
        };
        {
            let _step = begin("render pipeline", "Menu");
            assert_eq!(read(&mut out), "in render pipeline 'Menu'");
        }
        assert_eq!(read(&mut out), "after render pipeline 'Menu' returned");
        assert_eq!(named(None), "unlabeled");
    }
}
