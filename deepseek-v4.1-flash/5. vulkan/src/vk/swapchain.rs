//! Swapchain management: format / present-mode selection, per-image views and
//! framebuffers, and the per-image `render_finished` semaphores that make
//! semaphore reuse safe when `VK_KHR_present_wait` is unavailable.

use ash::vk;

use crate::error::{msg, Result};
use crate::vk::{Context, ResourceKind};

pub struct Swapchain {
    pub loader: ash::khr::swapchain::Device,
    pub handle: vk::SwapchainKHR,
    pub format: vk::Format,
    pub color_space: vk::ColorSpaceKHR,
    pub present_mode: vk::PresentModeKHR,
    pub extent: vk::Extent2D,
    pub images: Vec<vk::Image>,
    pub views: Vec<vk::ImageView>,
    pub framebuffers: Vec<vk::Framebuffer>,
    /// One semaphore per swapchain image: signalled when rendering into that
    /// image finishes, waited on by the presentation engine.
    pub render_finished: Vec<vk::Semaphore>,
    /// True when the chosen format performs the sRGB transfer function itself.
    pub srgb: bool,
}

impl Swapchain {
    pub fn create(
        ctx: &Context,
        surface: vk::SurfaceKHR,
        surface_loader: &ash::khr::surface::Instance,
        render_pass: vk::RenderPass,
        desired: vk::Extent2D,
        old: Option<vk::SwapchainKHR>,
    ) -> Result<Self> {
                let capabilities = unsafe {
            surface_loader.get_physical_device_surface_capabilities(ctx.physical_device, surface)?
        };
        if !capabilities
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        {
            return msg("the surface does not support COLOR_ATTACHMENT swapchain usage");
        }

                let formats =
            unsafe { surface_loader.get_physical_device_surface_formats(ctx.physical_device, surface)? };
        if formats.is_empty() {
            return msg("the surface reports no supported formats");
        }
        let chosen = formats
            .iter()
            .find(|f| {
                f.format == vk::Format::B8G8R8A8_SRGB
                    && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .or_else(|| {
                formats.iter().find(|f| {
                    f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
                        && f.format == vk::Format::R8G8B8A8_SRGB
                })
            })
            .or_else(|| {
                formats
                    .iter()
                    .find(|f| f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
            })
            .unwrap_or(&formats[0]);
        let format = chosen.format;
        let color_space = chosen.color_space;
        let srgb = matches!(
            format,
            vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB
        );

                let present_modes = unsafe {
            surface_loader.get_physical_device_surface_present_modes(ctx.physical_device, surface)?
        };
        // FIFO is the only mode guaranteed by the spec; MAILBOX avoids tearing
        // without throttling, so prefer it when the driver offers it.
        let present_mode = if present_modes.contains(&vk::PresentModeKHR::MAILBOX) {
            vk::PresentModeKHR::MAILBOX
        } else {
            vk::PresentModeKHR::FIFO
        };

        let extent = if capabilities.current_extent.width != u32::MAX {
            capabilities.current_extent
        } else {
            vk::Extent2D {
                width: desired
                    .width
                    .clamp(capabilities.min_image_extent.width, capabilities.max_image_extent.width),
                height: desired.height.clamp(
                    capabilities.min_image_extent.height,
                    capabilities.max_image_extent.height,
                ),
            }
        };
        if extent.width == 0 || extent.height == 0 {
            return msg("swapchain extent is zero (window minimized)");
        }
        if extent.width > ctx.info.max_framebuffer_size[0]
            || extent.height > ctx.info.max_framebuffer_size[1]
        {
            return msg(format!(
                "requested extent {}x{} exceeds maxFramebufferWidth/Height {:?}",
                extent.width, extent.height, ctx.info.max_framebuffer_size
            ));
        }

                let mut image_count = capabilities.min_image_count + 1;
        if capabilities.max_image_count > 0 {
            image_count = image_count.min(capabilities.max_image_count);
        }

        let composite_alpha = if capabilities
            .supported_composite_alpha
            .contains(vk::CompositeAlphaFlagsKHR::OPAQUE)
        {
            vk::CompositeAlphaFlagsKHR::OPAQUE
        } else {
            // Fall back to the first supported mode rather than failing.
            vk::CompositeAlphaFlagsKHR::from_raw(
                1 << capabilities.supported_composite_alpha.as_raw().trailing_zeros(),
            )
        };

        let create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(format)
            .image_color_space(color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(composite_alpha)
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(old.unwrap_or(vk::SwapchainKHR::null()));

        let loader = ash::khr::swapchain::Device::new(&ctx.instance, &ctx.device);
        let handle = unsafe { loader.create_swapchain(&create_info, None)? };
        ctx.resources.created(ResourceKind::Swapchain);
        let images = unsafe { loader.get_swapchain_images(handle)? };

        let mut views = Vec::with_capacity(images.len());
        let mut framebuffers = Vec::with_capacity(images.len());
        let mut render_finished = Vec::with_capacity(images.len());
        for image in &images {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            let view = unsafe { ctx.device.create_image_view(&view_info, None)? };
            ctx.resources.created(ResourceKind::ImageView);
            let attachments = [view];
            let framebuffer_info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            let framebuffer = unsafe { ctx.device.create_framebuffer(&framebuffer_info, None)? };
            ctx.resources.created(ResourceKind::Framebuffer);
            let semaphore = ctx.create_semaphore()?;
            views.push(view);
            framebuffers.push(framebuffer);
            render_finished.push(semaphore);
        }

        Ok(Self {
            loader,
            handle,
            format,
            color_space,
            present_mode,
            extent,
            images,
            views,
            framebuffers,
            render_finished,
            srgb,
        })
    }

    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    pub fn present_mode_name(&self) -> &'static str {
        match self.present_mode {
            vk::PresentModeKHR::MAILBOX => "MAILBOX",
            vk::PresentModeKHR::FIFO => "FIFO",
            vk::PresentModeKHR::IMMEDIATE => "IMMEDIATE",
            _ => "OTHER",
        }
    }

    pub fn format_name(&self) -> String {
        format!("{:?} / {:?}", self.format, self.color_space)
    }

    pub fn destroy(&self, ctx: &Context) {
        let device = &ctx.device;
        unsafe {
            for framebuffer in &self.framebuffers {
                device.destroy_framebuffer(*framebuffer, None);
            }
            for view in &self.views {
                device.destroy_image_view(*view, None);
            }
            self.loader.destroy_swapchain(self.handle, None);
        }
        for semaphore in &self.render_finished {
            ctx.destroy_semaphore(*semaphore);
        }
        for _ in &self.framebuffers {
            ctx.resources.destroyed(ResourceKind::Framebuffer);
        }
        for _ in &self.views {
            ctx.resources.destroyed(ResourceKind::ImageView);
        }
        ctx.resources.destroyed(ResourceKind::Swapchain);
    }
}
