extern crate wgpu;
extern crate winit;
use egui_pigeon::{setup, EguiPipe, ScreenDescriptor};
use euclid::Size2D;
use euclid::Transform3D;
use parrot::{painter::PassOp, transform::*, Painter, Plumber};
use pigeon_2d::pigeon;
use pigeon_2d::{pigeon::OPENGL_TO_WGPU_MATRIX, pipeline::Render};
use pigeon_parrot as parrot;
use pollster::FutureExt;
use std::time::Instant;
use winit::event::{Event, WindowEvent};
use winit::event_loop::ControlFlow;

pigeon!( | | EguiPipe >> setup => egui);

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .init();

    // Create an event loop
    let event_loop = winit::event_loop::EventLoop::new();
    // Create a window to draw to
    let window = winit::window::WindowBuilder::new()
        .with_title("Egui demo")
        .build(&event_loop)
        .unwrap();

    // Create a wgpu instance
    let instance = wgpu::Instance::new(wgpu::Backends::VULKAN);
    let surface = unsafe { instance.create_surface(&window) };

    // Get the size of the window
    let winsize = window.inner_size();

    let mut p = Pigeon::new(
        surface,
        &instance,
        Size2D::new(winsize.width as f32, winsize.height as f32),
        1,
    );

    let mut state = egui_winit::State::new(
        wgpu::Limits::default()
            .max_texture_dimension_2d
            .try_into()
            .unwrap(),
        &window,
    );

    let ctx = egui::Context::default();
    let mut demo_windows = egui_demo_lib::DemoWindows::default();

    event_loop.run(move |event, _, control_flow| match event {
        Event::WindowEvent {
            window_id: _,
            event: win_event,
            ..
        } => match win_event {
            WindowEvent::CloseRequested => {
                println!("The close button was pressed; stopping");
                *control_flow = ControlFlow::Exit;
            }
            WindowEvent::Resized(size) => {
                let size = euclid::Size2D::new(size.width, size.height);
                p.paint
                    .configure(size, wgpu::PresentMode::Fifo, wgpu::TextureFormat::Bgra8UnormSrgb);
                let size = euclid::Size2D::new(size.width as f32, size.height as f32);
                p.update_size(size);
            }
            _ => {
                window.request_redraw();
            }
        },
        Event::RedrawRequested(_) => {
            let raw_input = state.take_egui_input(&window);
            let sd = ScreenDescriptor {
                size_in_pixels: [p.screen.width as u32, p.screen.height as u32],
                pixels_per_point: state.pixels_per_point(),
            };
            let full_output = ctx.run(raw_input, |ctx| {
                demo_windows.ui(&ctx);
            });
            state.handle_platform_output(&window, &ctx, full_output.platform_output);
            let clipped_primatives = ctx.tessellate(full_output.shapes);
            p.egui.prepare(
                (full_output.textures_delta, clipped_primatives, sd),
                &mut p.paint,
            );
            custom_render::draw_cust(
                &mut p,
                false,
                |_| {},
                |p, _c, mut pass, _ob| {p.egui.render(&mut p.paint, &mut pass);}
            );
        }
        _ => (),
    });
}
