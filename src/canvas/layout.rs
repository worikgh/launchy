use super::*;
use crate::Color;
use std::collections::HashMap;

#[derive(Debug, Eq, PartialEq, Hash, Copy, Clone)]
pub enum Rotation {
	None,
	Left,
	Right,
	UpsideDown
}

impl Default for Rotation {
	fn default() -> Self { Self::None }
}

impl std::ops::Neg for Rotation {
	type Output = Self;

	fn neg(self) -> Self {
		match self {
			Self::None => Self::None,
			Self::UpsideDown => Self::UpsideDown,
			Self::Left => Self::Right,
			Self::Right => Self::Left
		}
	}
}

impl Rotation {
	pub fn translate(self, x: i32, y: i32) -> (i32, i32) {
		match self {
			Self::None => (x, y),
			Self::UpsideDown => (-x, -y),
			Self::Left => (-y, x),
			Self::Right => (y, -x),
		}
	}
}

struct LayoutDevice<'a> {
	canvas: Box<dyn Canvas + 'a>,
	rotation: Rotation,
	x: u32,
	y: u32,
}

unsafe impl Sync for LayoutDevice<'_> {} // fuck it

fn to_local(x: u32, y: u32, rot: Rotation, x_offset: u32, y_offset: u32) -> (u32, u32) {
	let x = x as i32;
	let y = y as i32;

	let (x, y) = (-rot).translate(x - x_offset as i32, y - y_offset as i32);

	(x as u32, y as u32)
}

fn to_global(x: u32, y: u32, rot: Rotation, x_offset: u32, y_offset: u32) -> (u32, u32) {
	let x = x as i32;
	let y = y as i32;


	let (x, y) = rot.translate(x, y);
	let (x, y) = (x + x_offset as i32, y + y_offset as i32);

	(x as u32, y as u32)
}

impl LayoutDevice<'_> {
	fn to_local(&self, x: u32, y: u32) -> (u32, u32) {
		to_local(x, y, self.rotation, self.x, self.y)
	}

	fn to_global(&self, x: u32, y: u32) -> (u32, u32) {
		to_global(x, y, self.rotation, self.x, self.y)
	}
}

/// Utility to be able to process messages from a CanvasLayout by polling
pub struct CanvasLayoutPoller {
	receiver: std::sync::mpsc::Receiver<CanvasMessage>,
}

impl crate::MsgPollingWrapper for CanvasLayoutPoller {
	type Message = CanvasMessage;

	fn receiver(&self) -> &std::sync::mpsc::Receiver<Self::Message> { &self.receiver }
}

struct Pixel {
	device_index: usize,
	actual_color: Color,
	actual_color_old: Color,
}

fn transform_color(color: Color, source: f32, target: f32) -> Color {
	// this is math :ghost:
	// and it doesn't work :ghost: nvm it does now
	(color - 1.0) * (1.0 - target) / (1.0 - source) + 1.0
}

/// Imagine this - you have multiple launchpads, you line them up, and now you use the Launchpads
/// as if they were a single device?! You can do that, with `CanvasLayout`.
/// 
/// Create a layout, add `Canvas`es to it at the position where they appear on your table, and
/// you're ready to rock!
/// 
/// Example:
/// ```rust
/// let mut canvas_layout = CanvasLayout::new(|msg| println!("Got a message: {:?}", msg));
/// 
/// // Assuming you have a Launchpad MK2 and a Launchpad S lying next to it:
/// canvas_layout.add_by_guess::<launchy::mk2::Canvas>(0, 0);
/// canvas_layout.add_by_guess::<launchy::s::Canvas>(9, 0);
/// 
/// // Light the entire canvas layout red - i.e. both Launchpads will be red
/// for button in canvas_layout.iter() {
/// 	button.set(&mut canvas_layout, launchy::Color::RED);
/// }
/// ```
pub struct CanvasLayout<'a> {
	devices: Vec<LayoutDevice<'a>>,
	coordinate_map: HashMap<(u32, u32), Pixel>, // we need to store some stuff for each pixel
	callback: std::sync::Arc<Box<dyn Fn(CanvasMessage) + Send + Sync + 'a>>,
	light_threshold: f32,
}

impl<'a> CanvasLayout<'a> {
	/// Create a new CanvasLayout that sends messages to the provided callback. The callback must
	/// implement `Fn` because it may be called from multiple devices concurrently.
	pub fn new(callback: impl Fn(CanvasMessage) + Send + Sync + 'a) -> Self {
		return Self {
			devices: Vec::with_capacity(10), // HACKJOB HACKJOB HACKJOB I NEED TO PREVENT REALLOCATIONS SO THAT THE CALLBACK WRAPPER DOESNT READ FROM UNINITIALIZED MEM so 10 ought to be enough hopefully
			coordinate_map: HashMap::new(),
			callback: std::sync::Arc::new(Box::new(callback)),
			light_threshold: 1.0 / 4.0, // good default value? I have, like, no idea
		};
	}

	/// Create a new CanvasLayout, plus an input handler object that you can use to poll messages.
	pub fn new_polling() -> (Self, CanvasLayoutPoller) {
		let (sender, receiver) = std::sync::mpsc::sync_channel(50);
		let canvas = Self::new(move |msg| sender.send(msg)
				.expect("Message receiver has hung up (this shouldn't happen)"));
		
		let poller = CanvasLayoutPoller { receiver };

		(canvas, poller)
	}

	pub fn light_threshold(&self) -> f32 { self.light_threshold }
	pub fn set_light_threshold(&mut self, value: f32) { self.light_threshold = value }

	/// Add a new device to this canvas layout, at the specified `x` and `y` coordinate.
	/// 
	/// The usage of this method is a bit awkward out of necessity. You need to provide a closure
	/// which, when called with a message callback, is expected to return a `Canvas` that is set up
	/// to deliver messsages to the provided message callback.
	/// 
	/// Any `Error`s from the closure will be propagated.
	/// 
	/// Example:
	/// ```rust
	/// canvas_layout.add(0, 0, |callback| launchy::mk2::Canvas::guess(callback))?;
	/// 
	/// // or even nested layouts:
	/// canvas_layout.add(0, 0, |callback| {
	/// 	let mut inner_canvas_layout = CanvasLayout::new(callback);
	/// 	inner_canvas_layout.add(0, 0, |inner_callback| launchy::mk2::Canvas::guess(inner_callback))
	/// })?;
	/// ```
	/// 
	/// If you want an easier way to add simple devices, see `add_by_guess`.
	pub fn add<C: 'a + Canvas, F, E>(&mut self,
		x_offset: u32,
		y_offset: u32,
		rotation: Rotation,
		creator: F
	) -> Result<(), E>
		where F: FnOnce(Box<dyn Fn(CanvasMessage) + Send + 'a>) -> Result<C, E> {

		let callback = self.callback.clone();
		let layout_device_container: std::sync::Arc<std::sync::Mutex<Option<&LayoutDevice>>> = std::sync::Arc::new(std::sync::Mutex::new(None));
		let layout_device_container_inner = layout_device_container.clone();
		let canvas = (creator)(Box::new(move |msg| {
			let layout_device = if let Some(a) = *layout_device_container_inner.lock().unwrap() { a } else { return };

			let (x, y) = layout_device.to_global(msg.x(), msg.y());
			match msg {
				CanvasMessage::Press { .. } => (callback)(CanvasMessage::Press { x, y }),
				CanvasMessage::Release { .. } => (callback)(CanvasMessage::Release { x, y }),
			}
		}))?;
		
		let index = self.devices.len(); // The index of soon-to-be inserted object
		
		for btn in canvas.iter() {
			let translated_coords = to_global(btn.x(), btn.y(), rotation, x_offset, y_offset);
			let old_value = self.coordinate_map.insert(translated_coords, Pixel {
				device_index: index,
				actual_color: btn.get(&canvas),
				actual_color_old: btn.get_old(&canvas),
			});
			
			// check for overlap
			if let Some(Pixel { device_index: old_device_index, .. }) = old_value {
				panic!(
					"Canvas is overlapping with canvas {} (zero-indexed) at ({}|{})!",
					old_device_index, translated_coords.0, translated_coords.1
				);
			}
		}
		
		let layout_device = LayoutDevice {
			canvas: Box::new(canvas),
			rotation, x: x_offset, y: y_offset
		};
		self.devices.push(layout_device);

		// TODO: this Arc<Mutex> thing is a very hacky solution
		*layout_device_container.lock().unwrap() = Some(unsafe {
			&*(&self.devices[self.devices.len() - 1] as *const LayoutDevice)
		});

		return Ok(());
	}

	/// Add a new device to this canvas, at the specified `x` and `y` coordinates. The MIDI
	/// connections used for communication with the underlying hardware are determined by guessing
	/// based on the device name.
	/// 
	/// Specifiy the type of device using a generic Canvas type parameter.
	/// 
	/// Example
	/// ```rust
	/// // Assuming a Launchpad MK2 and a Launchpad S next to it:
	/// canvas_layout.add_by_guess::<launchy::mk2::Canvas>(0, 0);
	/// canvas_layout.add_by_guess::<launchy::s::Canvas>(9, 0);
	/// ```
	pub fn add_by_guess<E: 'a + DeviceCanvasTrait>(&mut self,
		x: u32, y: u32,
	) -> anyhow::Result<()> {

		self.add(x, y, Rotation::None, DeviceCanvas::<E::Spec>::guess)
	}

	/// Like `add_by_guess`, but with a parameter for the rotation of the Launchpad.
	pub fn add_by_guess_rotated<E: 'a + DeviceCanvasTrait>(&mut self,
		x: u32, y: u32, rotation: Rotation,
	) -> anyhow::Result<()> {

		self.add(x, y, rotation, DeviceCanvas::<E::Spec>::guess)
	}
}

impl Canvas for CanvasLayout<'_> {
	fn lowest_visible_brightness(&self) -> f32 { self.light_threshold }

	fn bounding_box_width(&self) -> u32 {
		return self.devices.iter()
				.map(|device| device.x + device.canvas.bounding_box_width())
				.max().unwrap_or(0);
	}
	
	fn bounding_box_height(&self) -> u32 {
		return self.devices.iter()
				.map(|device| device.y + device.canvas.bounding_box_height())
				.max().unwrap_or(0);
	}
	
	fn is_valid(&self, x: u32, y: u32) -> bool {
		return self.coordinate_map.contains_key(&(x, y));
	}
	
	fn get_unchecked(&self, x: u32, y: u32) -> Color {
		let pixel = self.coordinate_map.get(&(x, y)).unwrap();
		pixel.actual_color
	}
	
	fn set_unchecked(&mut self, x: u32, y: u32, color: Color) {
		// store the actual pixel color for possible retrieval later
		let mut pixel = self.coordinate_map.get_mut(&(x, y)).unwrap();
		pixel.actual_color = color;

		let device = &mut self.devices[pixel.device_index];

		// but send the calibrated version to the actual underlying device
		let transformed_color = transform_color(
			color,
			self.light_threshold,
			device.canvas.lowest_visible_brightness(),
		);
		let (local_x, local_y) = device.to_local(x, y);
		device.canvas.set_unchecked(local_x, local_y, transformed_color);
	}
	
	fn get_old_unchecked(&self, x: u32, y: u32) -> Color {
		let pixel = self.coordinate_map.get(&(x, y)).unwrap();
		pixel.actual_color_old
	}
	
	fn flush(&mut self) -> anyhow::Result<()> {
		for device in &mut self.devices {
			device.canvas.flush()?;
		}

		for pixel in self.coordinate_map.values_mut() {
			pixel.actual_color_old = pixel.actual_color;
		}

		return Ok(());
	}
}