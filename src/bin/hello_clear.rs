#![allow(clippy::single_match)]
#![allow(clippy::len_zero)]

#[cfg(feature = "dx12")]
extern crate gfx_backend_dx12 as back;
#[cfg(feature = "metal")]
extern crate gfx_backend_metal as back;
#[cfg(feature = "vulkan")]
extern crate gfx_backend_vulkan as back;

use gfx_hal::{
  adapter::PhysicalDevice,
  command::{ClearColor, ClearValue, CommandBuffer, MultiShot, Primary},
  device::Device,
  format::{Aspects, ChannelType, Format, Swizzle},
  image::{Extent, Layout, SubresourceRange, ViewKind},
  pass::{Attachment, AttachmentLoadOp, AttachmentOps, AttachmentStoreOp, SubpassDesc},
  pool::{CommandPool, CommandPoolCreateFlags},
  pso::{PipelineStage, Rect},
  queue::{capability::Capability, Submission},
  window::{Backbuffer, FrameSync, Swapchain, SwapchainConfig},
  Backend, Gpu, Graphics, Instance, QueueFamily, Surface,
};
use winit::{dpi::LogicalSize, CreationError, Event, EventsLoop, Window, WindowBuilder, WindowEvent};

pub const WINDOW_NAME: &str = "Hello Clear";
const MAX_FRAMES_IN_FLIGHT: usize = 2;

fn main() {
  env_logger::init();

  let mut winit_state = WinitState::default();

  let instance = back::Instance::create(WINDOW_NAME, 1);
  let mut surface = instance.create_surface(&winit_state.window);
  let adapter = instance
    .enumerate_adapters()
    .into_iter()
    .find(|a| {
      a.queue_families
        .iter()
        .any(|qf| qf.supports_graphics() && qf.max_queues() > 0 && surface.supports_queue_family(qf))
    })
    .expect("Couldn't find a graphical Adapter!");
  let (device, mut command_queues, queue_type, qf_id) = {
    let queue_family = adapter
      .queue_families
      .iter()
      .find(|qf| qf.supports_graphics() && qf.max_queues() > 0 && surface.supports_queue_family(qf))
      .expect("Couldn't find a QueueFamily with graphics!");
    let Gpu { device, mut queues } = unsafe {
      adapter
        .physical_device
        .open(&[(&queue_family, &[1.0; 1])])
        .expect("Couldn't open the PhysicalDevice!")
    };
    let queue_group = queues
      .take::<Graphics>(queue_family.id())
      .expect("Couldn't take ownership of the QueueGroup!");
    debug_assert!(queue_group.queues.len() > 0);
    (device, queue_group.queues, queue_family.queue_type(), queue_family.id())
  };
  // DESCRIBE
  let (mut swapchain, extent, backbuffer, format) = {
    let (caps, formats, _present_modes, _composite_alphas) = surface.compatibility(&adapter.physical_device);
    let format = formats.map_or(Format::Rgba8Srgb, |formats| {
      formats
        .iter()
        .find(|format| format.base_format().1 == ChannelType::Srgb)
        .cloned()
        .unwrap_or(*formats.get(0).expect("Empty formats list specified!"))
    });
    let swap_config = SwapchainConfig::from_caps(&caps, format, caps.extents.end);
    let extent = swap_config.extent;
    let (swapchain, backbuffer) = unsafe {
      device
        .create_swapchain(&mut surface, swap_config, None)
        .expect("Failed to create the swapchain!")
    };
    (swapchain, extent, backbuffer, format)
  };
  // DESCRIBE
  let frame_images: Vec<(<back::Backend as Backend>::Image, <back::Backend as Backend>::ImageView)> = match backbuffer {
    Backbuffer::Images(images) => images
      .into_iter()
      .map(|image| {
        let image_view = unsafe {
          device
            .create_image_view(
              &image,
              ViewKind::D2,
              format,
              Swizzle::NO,
              SubresourceRange {
                aspects: Aspects::COLOR,
                levels: 0..1,
                layers: 0..1,
              },
            )
            .expect("Couldn't create the image_view for the image!")
        };
        (image, image_view)
      })
      .collect(),
    Backbuffer::Framebuffer(_) => unimplemented!("Can't handle framebuffer backbuffer!"),
  };
  // DESCRIBE
  let render_pass = {
    let color_attachment = Attachment {
      format: Some(format),
      samples: 1,
      ops: AttachmentOps {
        load: AttachmentLoadOp::Clear,
        store: AttachmentStoreOp::Store,
      },
      stencil_ops: AttachmentOps::DONT_CARE,
      layouts: Layout::Undefined..Layout::Present,
    };
    let subpass = SubpassDesc {
      colors: &[(0, Layout::ColorAttachmentOptimal)],
      depth_stencil: None,
      inputs: &[],
      resolves: &[],
      preserves: &[],
    };
    unsafe {
      device
        .create_render_pass(&[color_attachment], &[subpass], &[])
        .expect("Couldn't create a render pass!")
    }
  };
  // DESCRIBE
  let swapchain_framebuffers: Vec<<back::Backend as Backend>::Framebuffer> = {
    frame_images
      .iter()
      .map(|(_, image_view)| unsafe {
        device
          .create_framebuffer(
            &render_pass,
            vec![image_view],
            Extent {
              width: extent.width as _,
              height: extent.height as _,
              depth: 1,
            },
          )
          .expect("Failed to create a framebuffer!")
      })
      .collect()
  };
  // DESCRIBE
  let mut command_pool = {
    let raw_command_pool = unsafe {
      device
        .create_command_pool(qf_id, CommandPoolCreateFlags::empty())
        .expect("Could not create the raw command pool!")
    };
    assert!(Graphics::supported_by(queue_type));
    unsafe { CommandPool::<back::Backend, Graphics>::new(raw_command_pool) }
  };
  // DESCRIBE
  let submission_command_buffers: Vec<_> = unsafe {
    swapchain_framebuffers
      .iter()
      .map(|fb| {
        let mut command_buffer: CommandBuffer<back::Backend, Graphics, MultiShot, Primary> = command_pool.acquire_command_buffer();
        command_buffer.begin(true);
        let render_area = Rect {
          x: 0,
          y: 0,
          w: extent.width as i16,
          h: extent.height as i16,
        };
        let cornflower_blue = [0.259, 0.259, 0.435, 1.0];
        let clear_values = [ClearValue::Color(ClearColor::Float(cornflower_blue))];
        command_buffer.begin_render_pass_inline(&render_pass, fb, render_area, clear_values.iter());
        command_buffer.finish();
        command_buffer
      })
      .collect()
  };
  // DESCRIBE
  let (image_available_semaphores, render_finished_semaphores, in_flight_fences) = {
    let mut image_available_semaphores: Vec<<back::Backend as Backend>::Semaphore> = vec![];
    let mut render_finished_semaphores: Vec<<back::Backend as Backend>::Semaphore> = vec![];
    let mut in_flight_fences: Vec<<back::Backend as Backend>::Fence> = vec![];
    for _ in 0..MAX_FRAMES_IN_FLIGHT {
      image_available_semaphores.push(device.create_semaphore().expect("Could not create a semaphore!"));
      render_finished_semaphores.push(device.create_semaphore().expect("Could not create a semaphore!"));
      in_flight_fences.push(device.create_fence(true).expect("Could not create a fence!"));
    }
    (image_available_semaphores, render_finished_semaphores, in_flight_fences)
  };

  //

  let mut current_frame = 0;

  let mut running = true;
  while running {
    winit_state.events_loop.poll_events(|event| match event {
      Event::WindowEvent {
        event: WindowEvent::CloseRequested,
        ..
      } => running = false,
      _ => (),
    });
    if !running {
      device.wait_idle().expect("Queues aren't going to idle!");
      break;
    }

    // Draw a frame
    unsafe {
      device
        .wait_for_fence(&in_flight_fences[current_frame], std::u64::MAX)
        .expect("Failed to wait on the fence!");
      device.reset_fence(&in_flight_fences[current_frame]).expect("Couldn't reset the fence!");
      let image_index = swapchain
        .acquire_image(std::u64::MAX, FrameSync::Semaphore(&image_available_semaphores[current_frame]))
        .expect("Couldn't acquire an image from the swapchain!");
      let i = image_index as usize;
      let submission = Submission {
        command_buffers: &submission_command_buffers[i..=i],
        wait_semaphores: vec![(&image_available_semaphores[current_frame], PipelineStage::COLOR_ATTACHMENT_OUTPUT)],
        signal_semaphores: vec![&render_finished_semaphores[current_frame]],
      };
      command_queues[0].submit(submission, Some(&in_flight_fences[current_frame]));
      swapchain
        .present(&mut command_queues[0], image_index, vec![&render_finished_semaphores[current_frame]])
        .expect("Couldn't present the image!");
    }

    current_frame = (current_frame + 1) % MAX_FRAMES_IN_FLIGHT;
  }

  // TODO: Theoretically one could do cleanup here.
}

#[derive(Debug)]
pub struct WinitState {
  pub events_loop: EventsLoop,
  pub window: Window,
}
impl WinitState {
  /// Constructs a new `EventsLoop` and `Window` pair.
  ///
  /// The specified title and size are used, other elements are default.
  /// ## Failure
  /// It's possible for the window creation to fail. This is unlikely.
  pub fn new<T: Into<String>>(title: T, size: LogicalSize) -> Result<Self, CreationError> {
    let events_loop = EventsLoop::new();
    let output = WindowBuilder::new().with_title(title).with_dimensions(size).build(&events_loop);
    output.map(|window| Self { events_loop, window })
  }
}
impl Default for WinitState {
  /// Makes an 800x600 window with the `WINDOW_NAME` value as the title.
  /// ## Panics
  /// If a `CreationError` occurs.
  fn default() -> Self {
    Self::new(WINDOW_NAME, LogicalSize { width: 800.0, height: 600.0 }).expect("Could not create a window!")
  }
}
