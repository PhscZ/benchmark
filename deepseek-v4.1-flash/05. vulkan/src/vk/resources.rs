//! GPU buffer / image / sampler wrappers with explicit destruction.
//!
//! Every wrapper exposes a `destroy(&ash::Device)`; the owning renderer calls
//! them from its `Drop` (before the [`crate::vk::Context`] is dropped), so
//! repeated swapchain recreation and render restarts cannot leak device memory.

use ash::vk;

use crate::error::{msg, Result};
use crate::vk::{Context, ResourceKind};

pub struct GpuBuffer {
    pub buffer: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: u64,
    mapped: Option<*mut u8>,
    host_visible: bool,
}

impl GpuBuffer {
    fn create(
        ctx: &Context,
        size: u64,
        usage: vk::BufferUsageFlags,
        properties: vk::MemoryPropertyFlags,
    ) -> Result<Self> {
        let info = vk::BufferCreateInfo::default()
            .size(size.max(1))
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { ctx.device.create_buffer(&info, None)? };
        ctx.resources.created(ResourceKind::Buffer);
        let requirements = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
        let memory = match ctx.allocate_memory(&requirements, properties) {
            Ok(memory) => memory,
            Err(e) => {
                unsafe { ctx.device.destroy_buffer(buffer, None) };
                ctx.resources.destroyed(ResourceKind::Buffer);
                return Err(e);
            }
        };
        if let Err(e) = unsafe { ctx.device.bind_buffer_memory(buffer, memory, 0) } {
            unsafe {
                ctx.device.destroy_buffer(buffer, None);
                ctx.device.free_memory(memory, None);
            }
            // The counters were never incremented for these, so nothing to undo.
            return Err(e.into());
        }
        let mapped = if properties.contains(vk::MemoryPropertyFlags::HOST_VISIBLE) {
            let ptr = unsafe {
                ctx.device
                    .map_memory(memory, 0, requirements.size, vk::MemoryMapFlags::empty())?
            };
            Some(ptr as *mut u8)
        } else {
            None
        };
        Ok(Self {
            buffer,
            memory,
            size,
            mapped,
            host_visible: mapped.is_some(),
        })
    }

    /// Device-local buffer (fast for shader reads).
    pub fn device_local(ctx: &Context, size: u64, usage: vk::BufferUsageFlags) -> Result<Self> {
        Self::create(
            ctx,
            size,
            usage,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
    }

    /// Host-visible, host-coherent buffer (uniform data, readback).
    pub fn host_visible(ctx: &Context, size: u64, usage: vk::BufferUsageFlags) -> Result<Self> {
        Self::create(
            ctx,
            size,
            usage,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
    }

    /// Device-local buffer initialised through a staging copy.
    pub fn upload(
        ctx: &Context,
        usage: vk::BufferUsageFlags,
        data: &[u8],
    ) -> Result<Self> {
        let size = data.len() as u64;
        let target = Self::device_local(ctx, size, usage | vk::BufferUsageFlags::TRANSFER_DST)?;
        if size == 0 {
            return Ok(target);
        }
        let staging = Self::host_visible(ctx, size, vk::BufferUsageFlags::TRANSFER_SRC)?;
        staging.write_bytes(data);
        let staging_buffer = staging.buffer;
        let target_buffer = target.buffer;
        let result = ctx.one_shot(|cmd| {
            let region = vk::BufferCopy::default().size(size);
            unsafe {
                ctx.device
                    .cmd_copy_buffer(cmd, staging_buffer, target_buffer, &[region]);
            }
        });
        staging.destroy(ctx);
        result?;
        Ok(target)
    }

    /// Copies `data` into a host-visible mapping.
    pub fn write_bytes(&self, data: &[u8]) {
        debug_assert!(data.len() as u64 <= self.size);
        let Some(ptr) = self.mapped else {
            panic!("write_bytes on a non-host-visible buffer");
        };
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        }
    }

    /// Reads `len` bytes back from a host-visible mapping.
    pub fn read_bytes(&self, len: usize) -> Vec<u8> {
        let Some(ptr) = self.mapped else {
            panic!("read_bytes on a non-host-visible buffer");
        };
        let mut out = vec![0u8; len.min(self.size as usize)];
        unsafe {
            std::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr(), out.len());
        }
        out
    }

    pub fn is_host_visible(&self) -> bool {
        self.host_visible
    }

    pub fn destroy(&self, ctx: &Context) {
        unsafe {
            if let Some(ptr) = self.mapped {
                ctx.device.unmap_memory(self.memory);
                let _ = ptr;
            }
            ctx.device.destroy_buffer(self.buffer, None);
            ctx.device.free_memory(self.memory, None);
        }
        ctx.resources.destroyed(ResourceKind::Buffer);
        ctx.resources.destroyed(ResourceKind::DeviceMemory);
    }
}

pub struct GpuImage {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    pub usage: vk::ImageUsageFlags,
}

impl GpuImage {
    pub fn new(
        ctx: &Context,
        format: vk::Format,
        extent: vk::Extent2D,
        usage: vk::ImageUsageFlags,
        aspect: vk::ImageAspectFlags,
    ) -> Result<Self> {
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: extent.width.max(1),
                height: extent.height.max(1),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { ctx.device.create_image(&info, None)? };
        ctx.resources.created(ResourceKind::Image);
        let requirements = unsafe { ctx.device.get_image_memory_requirements(image) };
        let memory = match ctx.allocate_memory(&requirements, vk::MemoryPropertyFlags::DEVICE_LOCAL) {
            Ok(memory) => memory,
            Err(e) => {
                unsafe { ctx.device.destroy_image(image, None) };
                ctx.resources.destroyed(ResourceKind::Image);
                return Err(e);
            }
        };
        if let Err(e) = unsafe { ctx.device.bind_image_memory(image, memory, 0) } {
            unsafe {
                ctx.device.destroy_image(image, None);
                ctx.device.free_memory(memory, None);
            }
            ctx.resources.destroyed(ResourceKind::Image);
            ctx.resources.destroyed(ResourceKind::DeviceMemory);
            return Err(e.into());
        }
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: aspect,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = match unsafe { ctx.device.create_image_view(&view_info, None) } {
            Ok(view) => view,
            Err(e) => {
                unsafe {
                    ctx.device.destroy_image(image, None);
                    ctx.device.free_memory(memory, None);
                }
                ctx.resources.destroyed(ResourceKind::Image);
                ctx.resources.destroyed(ResourceKind::DeviceMemory);
                return Err(e.into());
            }
        };
        ctx.resources.created(ResourceKind::ImageView);
        Ok(Self {
            image,
            memory,
            view,
            format,
            extent,
            usage,
        })
    }

    /// Uploads tightly packed pixel data into a freshly created image.
    pub fn upload(
        ctx: &Context,
        format: vk::Format,
        extent: vk::Extent2D,
        aspect: vk::ImageAspectFlags,
        data: &[u8],
        bytes_per_pixel: u32,
    ) -> Result<Self> {
        let image = Self::new(
            ctx,
            format,
            extent,
            vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
            aspect,
        )?;
        let size = (extent.width * extent.height * bytes_per_pixel) as u64;
        if size == 0 {
            return Ok(image);
        }
        if data.len() as u64 != size {
            image.destroy(ctx);
            return msg(format!(
                "image upload size mismatch: expected {size} bytes, got {}",
                data.len()
            ));
        }
        let staging = GpuBuffer::host_visible(ctx, size, vk::BufferUsageFlags::TRANSFER_SRC)?;
        staging.write_bytes(data);
        let staging_buffer = staging.buffer;
        let target = image.image;
        let result = ctx.one_shot(|cmd| {
            let to_transfer = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(target)
                .subresource_range(subresource_range(aspect))
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
            unsafe {
                ctx.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_transfer],
                );
                let region = vk::BufferImageCopy::default()
                    .image_subresource(vk::ImageSubresourceLayers {
                        aspect_mask: aspect,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .image_extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    });
                ctx.device.cmd_copy_buffer_to_image(
                    cmd,
                    staging_buffer,
                    target,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
                let to_read = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(target)
                    .subresource_range(subresource_range(aspect))
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ);
                ctx.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_read],
                );
            }
        });
        staging.destroy(ctx);
        result?;
        Ok(image)
    }

    pub fn destroy(&self, ctx: &Context) {
        unsafe {
            ctx.device.destroy_image_view(self.view, None);
            ctx.device.destroy_image(self.image, None);
            ctx.device.free_memory(self.memory, None);
        }
        ctx.resources.destroyed(ResourceKind::ImageView);
        ctx.resources.destroyed(ResourceKind::Image);
        ctx.resources.destroyed(ResourceKind::DeviceMemory);
    }
}

pub fn subresource_range(aspect: vk::ImageAspectFlags) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: aspect,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}

pub fn create_sampler(
    ctx: &Context,
    filter: vk::Filter,
    address: vk::SamplerAddressMode,
) -> Result<vk::Sampler> {
    let info = vk::SamplerCreateInfo::default()
        .mag_filter(filter)
        .min_filter(filter)
        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
        .address_mode_u(address)
        .address_mode_v(address)
        .address_mode_w(address)
        .min_lod(0.0)
        .max_lod(0.0);
    let sampler = unsafe { ctx.device.create_sampler(&info, None)? };
    ctx.resources.created(ResourceKind::Sampler);
    Ok(sampler)
}

/// Full-image barrier helper used for the accumulation image.
#[allow(clippy::too_many_arguments)]
pub fn image_barrier(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_stage: vk::PipelineStageFlags,
    dst_stage: vk::PipelineStageFlags,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(subresource_range(vk::ImageAspectFlags::COLOR))
        .src_access_mask(src_access)
        .dst_access_mask(dst_access);
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}
