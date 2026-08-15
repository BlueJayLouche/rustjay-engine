//! One serialized wrapper around the app-wide [`wgpu::Queue`].
//!
//! wgpu 30's DX12 backend stages external fence waits (`add_wait_fence`) that
//! are drained by whichever `Queue::submit` runs next. CuePool submits from
//! several threads (video-consume, per-output render threads, the winit/egui
//! thread, the pixel sampler), so a staged decode-fence wait could otherwise
//! be consumed by an unrelated thread's submission — leaving the zero-copy
//! conversion to sample a frame the decoder has not finished writing. Every
//! submission therefore goes through this wrapper, and the zero-copy path
//! stages its wait and submits inside one critical section.

use std::sync::Mutex;

/// A decode fence to wait on, captured from a specific decoded frame.
///
/// The value is the one FFmpeg will signal when that frame's decode completes;
/// per-frame pool resources reuse their fence with an incrementing value, so
/// the pair must always travel together.
#[cfg(windows)]
#[derive(Clone)]
pub struct DecodeFence {
    pub(crate) fence: windows::Win32::Graphics::Direct3D12::ID3D12Fence,
    pub(crate) value: u64,
}

#[cfg(windows)]
impl std::fmt::Debug for DecodeFence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DecodeFence")
            .field("value", &self.value)
            .finish_non_exhaustive()
    }
}

pub struct SharedQueue {
    queue: wgpu::Queue,
    submit_lock: Mutex<()>,
}

impl SharedQueue {
    pub fn new(queue: wgpu::Queue) -> Self {
        Self {
            queue,
            submit_lock: Mutex::new(()),
        }
    }

    /// The wrapped queue, for non-submitting operations (`write_texture`,
    /// `write_buffer`, `on_submitted_work_done`). Those stage work that the
    /// next serialized submit flushes; they never call the HAL submit
    /// themselves, so they are safe outside the lock.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Serialized plain submission.
    pub fn submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(
        &self,
        command_buffers: I,
    ) -> wgpu::SubmissionIndex {
        let _guard = self
            .submit_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.queue.submit(command_buffers)
    }

    /// Stages `fence` at `value` and submits within one critical section, so
    /// the wait is consumed by exactly this submission: no other thread's
    /// submission can drain it first, and this submission cannot run on the
    /// GPU before the decoder signals the fence.
    #[cfg(windows)]
    pub fn submit_with_decode_wait<I: IntoIterator<Item = wgpu::CommandBuffer>>(
        &self,
        fence: &DecodeFence,
        command_buffers: I,
    ) -> Result<wgpu::SubmissionIndex, String> {
        let _guard = self
            .submit_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        {
            let hal_queue = unsafe { self.queue.as_hal::<wgpu::hal::api::Dx12>() }
                .ok_or_else(|| "wgpu queue is not backed by DX12".to_string())?;
            hal_queue.add_wait_fence(fence.fence.clone(), fence.value);
        }
        Ok(self.queue.submit(command_buffers))
    }
}

impl std::fmt::Debug for SharedQueue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedQueue")
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::Graphics::Direct3D12::{D3D12_FENCE_FLAG_NONE, ID3D12Fence};

    fn dx12_shared_queue() -> Option<(wgpu::Device, Arc<SharedQueue>, ID3D12Fence)> {
        // The default adapter may be Vulkan; these tests are about DX12 fence
        // staging, so request the DX12 backend explicitly.
        let (device, queue) = pollster::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::DX12,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .ok()?;
            adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .ok()
        })?;
        let raw_device = {
            let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }?;
            hal_device.raw_device().clone()
        };
        let fence: ID3D12Fence =
            unsafe { raw_device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ok()?;
        Some((device, Arc::new(SharedQueue::new(queue)), fence))
    }

    fn empty_submission(device: &wgpu::Device) -> wgpu::CommandBuffer {
        device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default())
            .finish()
    }

    /// The association invariant: a decode-fence wait gates exactly the
    /// submission it was staged with. An earlier submission from another
    /// thread completes while the fence is unsignaled; the paired submission
    /// completes only after the fence signals.
    #[test]
    fn decode_wait_gates_its_own_submission_only() {
        let _gpu = crate::gpu_test_lock();
        let Some((device, shared, fence)) = dx12_shared_queue() else {
            eprintln!("skipping decode-wait test: no DX12 adapter available");
            return;
        };

        // A plain submission ahead of the wait pair must be unaffected.
        let before = shared.submit([empty_submission(&device)]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(before),
                timeout: Some(std::time::Duration::from_secs(5)),
            })
            .expect("submission ahead of the wait pair should complete");

        let decode_fence = DecodeFence {
            fence: fence.clone(),
            value: 1,
        };
        let gated = shared
            .submit_with_decode_wait(&decode_fence, [empty_submission(&device)])
            .expect("DX12 submit with decode wait");

        // The paired submission must NOT complete while the fence is
        // unsignaled.
        let timed_out = device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(gated.clone()),
                timeout: Some(std::time::Duration::from_millis(200)),
            })
            .is_err();
        assert!(
            timed_out,
            "submission paired with an unsignaled decode fence completed early"
        );

        unsafe { fence.Signal(1) }.expect("CPU fence signal");
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(gated),
                timeout: Some(std::time::Duration::from_secs(5)),
            })
            .expect("submission should complete once the decode fence signals");
    }

    /// Concurrent wrapper submissions cannot interleave a wait pair: the
    /// staged wait is always drained by its own submission, never left over
    /// for (or stolen by) another thread's submit.
    #[test]
    fn concurrent_submissions_do_not_steal_the_staged_wait() {
        let _gpu = crate::gpu_test_lock();
        let Some((device, shared, fence)) = dx12_shared_queue() else {
            eprintln!("skipping wait-steal test: no DX12 adapter available");
            return;
        };

        let stop = Arc::new(AtomicBool::new(false));
        let contender = {
            let shared = Arc::clone(&shared);
            let device = device.clone();
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                // Bounded and paced: enough traffic to contend with every main
                // round without flooding the queue (an unthrottled loop here
                // once burned 15 CPU-minutes across the suite).
                for _ in 0..500 {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    shared.submit([empty_submission(&device)]);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })
        };

        for round in 0..50u64 {
            let value = round + 1;
            // Pre-signal so every pair completes promptly; the invariant under
            // test is the drain pairing, not the gating.
            unsafe { fence.Signal(value) }.expect("CPU fence signal");
            let decode_fence = DecodeFence {
                fence: fence.clone(),
                value,
            };
            shared
                .submit_with_decode_wait(&decode_fence, [empty_submission(&device)])
                .expect("DX12 submit with decode wait");
            // If a contender submission had slipped between the stage and the
            // submit, the wait would still be pending here.
            let hal_queue =
                unsafe { shared.queue().as_hal::<wgpu::hal::api::Dx12>() }.expect("DX12 queue");
            assert!(
                !hal_queue.remove_wait_fence(&fence),
                "staged decode wait was not consumed by its paired submission"
            );
        }

        stop.store(true, Ordering::Relaxed);
        contender.join().expect("contender thread");
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("drain test submissions");
    }
}
