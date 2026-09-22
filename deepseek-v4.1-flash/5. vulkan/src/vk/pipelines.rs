//! Shader modules, descriptor layouts/sets, pipeline layouts, the presentation
//! render pass and all four pipelines (two compute, two graphics).
//!
//! Rasterization appears only in the presentation pass (fullscreen blit of the
//! ray-traced image) and the UI overlay; the image itself is always produced by
//! the compute shader.

use ash::vk;

use crate::error::{msg, Result};
use crate::vk::{Context, ResourceKind};

/// Push-constant block shared by the compute shaders (`shaders/raytrace.comp`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TracePushConstants {
    pub sample_base: u32,
    pub sample_count: u32,
    pub total_samples: u32,
    pub stratified: u32,
}

/// Push-constant block of `shaders/present.frag`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PresentPushConstants {
    pub inv_sample_count: f32,
    pub exposure: f32,
    pub encode_srgb: f32,
    pub pad: f32,
}

/// Push-constant block of `shaders/ui.vert`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiPushConstants {
    pub viewport: [f32; 2],
    pub pad: [f32; 2],
}

pub struct Pipelines {
    pub format: vk::Format,
    pub render_pass: vk::RenderPass,
    pub trace_set_layout: vk::DescriptorSetLayout,
    pub sampler_set_layout: vk::DescriptorSetLayout,
    pub trace_pipeline_layout: vk::PipelineLayout,
    pub present_pipeline_layout: vk::PipelineLayout,
    pub ui_pipeline_layout: vk::PipelineLayout,
    pub raytrace_pipeline: vk::Pipeline,
    pub intersect_test_pipeline: vk::Pipeline,
    pub present_pipeline: vk::Pipeline,
    pub ui_pipeline: vk::Pipeline,
    pub descriptor_pool: vk::DescriptorPool,
    pub trace_set: vk::DescriptorSet,
    pub present_set: vk::DescriptorSet,
    pub ui_set: vk::DescriptorSet,
    shader_modules: Vec<vk::ShaderModule>,
}

impl Pipelines {
    pub fn create(ctx: &Context, format: vk::Format) -> Result<Self> {
        let device = &ctx.device;

        // --- render pass (presentation only) --------------------------------
        let color_attachment = vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let color_reference = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_reference);
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        let attachments = [color_attachment];
        let subpasses = [subpass];
        let dependencies = [dependency];
        let render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        let render_pass = unsafe { device.create_render_pass(&render_pass_info, None)? };
        ctx.resources.created(ResourceKind::RenderPass);

        // --- descriptor set layouts -----------------------------------------
        let trace_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(5)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];
        let trace_set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&trace_bindings),
                None,
            )?
        };
        ctx.resources.created(ResourceKind::DescriptorSetLayout);
        // Separate sampled image + sampler (the GLSL front end's model).
        let sampler_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let sampler_set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&sampler_bindings),
                None,
            )?
        };
        ctx.resources.created(ResourceKind::DescriptorSetLayout);

        // --- pipeline layouts -----------------------------------------------
        let trace_layouts = [trace_set_layout];
        let trace_push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(std::mem::size_of::<TracePushConstants>() as u32)];
        let trace_pipeline_layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&trace_layouts)
                    .push_constant_ranges(&trace_push),
                None,
            )?
        };
        ctx.resources.created(ResourceKind::PipelineLayout);
        let sampler_layouts = [sampler_set_layout];
        let present_push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<PresentPushConstants>() as u32)];
        let present_pipeline_layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&sampler_layouts)
                    .push_constant_ranges(&present_push),
                None,
            )?
        };
        ctx.resources.created(ResourceKind::PipelineLayout);
        let ui_push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(std::mem::size_of::<UiPushConstants>() as u32)];
        let ui_pipeline_layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&sampler_layouts)
                    .push_constant_ranges(&ui_push),
                None,
            )?
        };
        ctx.resources.created(ResourceKind::PipelineLayout);

        // --- shader modules --------------------------------------------------
        let mut modules = Vec::new();
        let mut module = |bytes: &[u8], label: &str| -> Result<vk::ShaderModule> {
            let words = spirv_words(bytes);
            if words.is_empty() {
                return msg(format!("shader {label} is empty"));
            }
            let info = vk::ShaderModuleCreateInfo::default().code(&words);
            let handle = unsafe { device.create_shader_module(&info, None)? };
            ctx.resources.created(ResourceKind::ShaderModule);
            modules.push(handle);
            Ok(handle)
        };

        let raytrace_module =
            module(crate::shaders::RAYTRACE_COMP, "raytrace.comp")?;
        let intersect_module =
            module(crate::shaders::INTERSECT_TEST_COMP, "intersect_test.comp")?;
        let present_vert_module = module(crate::shaders::PRESENT_VERT, "present.vert")?;
        let present_frag_module = module(crate::shaders::PRESENT_FRAG, "present.frag")?;
        let ui_vert_module = module(crate::shaders::UI_VERT, "ui.vert")?;
        let ui_frag_module = module(crate::shaders::UI_FRAG, "ui.frag")?;

        // --- compute pipelines ------------------------------------------------
        let entry_point = c"main";
        let compute_stage = |module: vk::ShaderModule| {
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(module)
                .name(entry_point)
        };
        let raytrace_info = vk::ComputePipelineCreateInfo::default()
            .stage(compute_stage(raytrace_module))
            .layout(trace_pipeline_layout);
        let intersect_info = vk::ComputePipelineCreateInfo::default()
            .stage(compute_stage(intersect_module))
            .layout(trace_pipeline_layout);
        let compute_pipelines = unsafe {
            device.create_compute_pipelines(
                vk::PipelineCache::null(),
                &[raytrace_info, intersect_info],
                None,
            )
        }
        .map_err(|(_, e)| crate::error::Error::Vk(e))?;
        let raytrace_pipeline = compute_pipelines[0];
        let intersect_test_pipeline = compute_pipelines[1];
        ctx.resources.created(ResourceKind::Pipeline);
        ctx.resources.created(ResourceKind::Pipeline);

        // --- graphics pipelines ----------------------------------------------
        let vertex_input_empty = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let color_blend_attachments_disabled =
            [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(false)
                .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend_disabled = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(&color_blend_attachments_disabled);
        let color_blend_attachments_alpha = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend_alpha = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(&color_blend_attachments_alpha);
        let color_reference = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_reference);
        let _subpasses = [subpass];

        let graphics_stage = |module: vk::ShaderModule, stage: vk::ShaderStageFlags| {
            vk::PipelineShaderStageCreateInfo::default()
                .stage(stage)
                .module(module)
                .name(entry_point)
        };

        let present_stages = [
            graphics_stage(present_vert_module, vk::ShaderStageFlags::VERTEX),
            graphics_stage(present_frag_module, vk::ShaderStageFlags::FRAGMENT),
        ];
        let present_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&present_stages)
            .vertex_input_state(&vertex_input_empty)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend_disabled)
            .dynamic_state(&dynamic_state)
            .layout(present_pipeline_layout)
            .render_pass(render_pass)
            .subpass(0);

        let ui_binding = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<crate::ui::UiVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let ui_attributes = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .binding(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .binding(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(8),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .binding(0)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(16),
        ];
        let ui_vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&ui_binding)
            .vertex_attribute_descriptions(&ui_attributes);
        let ui_stages = [
            graphics_stage(ui_vert_module, vk::ShaderStageFlags::VERTEX),
            graphics_stage(ui_frag_module, vk::ShaderStageFlags::FRAGMENT),
        ];
        let ui_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&ui_stages)
            .vertex_input_state(&ui_vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend_alpha)
            .dynamic_state(&dynamic_state)
            .layout(ui_pipeline_layout)
            .render_pass(render_pass)
            .subpass(0);

        let graphics_pipelines = unsafe {
            device.create_graphics_pipelines(
                vk::PipelineCache::null(),
                &[present_info, ui_info],
                None,
            )
        }
        .map_err(|(_, e)| crate::error::Error::Vk(e))?;
        ctx.resources.created(ResourceKind::Pipeline);
        ctx.resources.created(ResourceKind::Pipeline);

        // --- descriptor pool + sets ------------------------------------------
        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(4),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(2),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLER)
                .descriptor_count(2),
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(3)
            .pool_sizes(&pool_sizes);
        let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None)? };
        ctx.resources.created(ResourceKind::DescriptorPool);

        let set_layouts = [trace_set_layout, sampler_set_layout, sampler_set_layout];
        let sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&set_layouts),
            )?
        };

        Ok(Self {
            format,
            render_pass,
            trace_set_layout,
            sampler_set_layout,
            trace_pipeline_layout,
            present_pipeline_layout,
            ui_pipeline_layout,
            raytrace_pipeline,
            intersect_test_pipeline,
            present_pipeline: graphics_pipelines[0],
            ui_pipeline: graphics_pipelines[1],
            descriptor_pool,
            trace_set: sets[0],
            present_set: sets[1],
            ui_set: sets[2],
            shader_modules: modules,
        })
    }

    /// Points the compute descriptor set at the current resources.  Binding 4/5
    /// (the intersection-test ray/result buffers) are always bound, so the same
    /// set layout serves both compute pipelines.
    #[allow(clippy::too_many_arguments)]
    pub fn update_trace_set(
        &self,
        ctx: &Context,
        accum_view: vk::ImageView,
        sphere_buffer: vk::Buffer,
        sphere_size: u64,
        triangle_buffer: vk::Buffer,
        triangle_size: u64,
        uniform_buffer: vk::Buffer,
        uniform_size: u64,
        test_ray_buffer: vk::Buffer,
        test_ray_size: u64,
        test_result_buffer: vk::Buffer,
        test_result_size: u64,
    ) {
        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(accum_view)
            .image_layout(vk::ImageLayout::GENERAL)];
        let sphere_info = [vk::DescriptorBufferInfo::default()
            .buffer(sphere_buffer)
            .offset(0)
            .range(sphere_size)];
        let triangle_info = [vk::DescriptorBufferInfo::default()
            .buffer(triangle_buffer)
            .offset(0)
            .range(triangle_size)];
        let uniform_info = [vk::DescriptorBufferInfo::default()
            .buffer(uniform_buffer)
            .offset(0)
            .range(uniform_size)];
        let test_ray_info = [vk::DescriptorBufferInfo::default()
            .buffer(test_ray_buffer)
            .offset(0)
            .range(test_ray_size)];
        let test_result_info = [vk::DescriptorBufferInfo::default()
            .buffer(test_result_buffer)
            .offset(0)
            .range(test_result_size)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.trace_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&image_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.trace_set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&sphere_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.trace_set)
                .dst_binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&triangle_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.trace_set)
                .dst_binding(3)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&uniform_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.trace_set)
                .dst_binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&test_ray_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.trace_set)
                .dst_binding(5)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&test_result_info),
        ];
        unsafe { ctx.device.update_descriptor_sets(&writes, &[]) };
    }

    pub fn update_present_set(
        &self,
        ctx: &Context,
        accum_view: vk::ImageView,
        sampler: vk::Sampler,
    ) {
        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(accum_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let sampler_info = [vk::DescriptorImageInfo::default().sampler(sampler)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.present_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&image_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.present_set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(&sampler_info),
        ];
        unsafe { ctx.device.update_descriptor_sets(&writes, &[]) };
    }

    pub fn update_ui_set(&self, ctx: &Context, atlas_view: vk::ImageView, sampler: vk::Sampler) {
        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(atlas_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let sampler_info = [vk::DescriptorImageInfo::default().sampler(sampler)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.ui_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&image_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.ui_set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(&sampler_info),
        ];
        unsafe { ctx.device.update_descriptor_sets(&writes, &[]) };
    }

    pub fn destroy(&self, ctx: &Context) {
        let device = &ctx.device;
        unsafe {
            device.destroy_pipeline(self.raytrace_pipeline, None);
            device.destroy_pipeline(self.intersect_test_pipeline, None);
            device.destroy_pipeline(self.present_pipeline, None);
            device.destroy_pipeline(self.ui_pipeline, None);
            device.destroy_pipeline_layout(self.trace_pipeline_layout, None);
            device.destroy_pipeline_layout(self.present_pipeline_layout, None);
            device.destroy_pipeline_layout(self.ui_pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.trace_set_layout, None);
            device.destroy_descriptor_set_layout(self.sampler_set_layout, None);
            device.destroy_render_pass(self.render_pass, None);
            for module in &self.shader_modules {
                device.destroy_shader_module(*module, None);
            }
        }
        ctx.resources.destroyed(ResourceKind::Pipeline);
        ctx.resources.destroyed(ResourceKind::Pipeline);
        ctx.resources.destroyed(ResourceKind::Pipeline);
        ctx.resources.destroyed(ResourceKind::Pipeline);
        ctx.resources.destroyed(ResourceKind::PipelineLayout);
        ctx.resources.destroyed(ResourceKind::PipelineLayout);
        ctx.resources.destroyed(ResourceKind::PipelineLayout);
        ctx.resources.destroyed(ResourceKind::DescriptorPool);
        ctx.resources.destroyed(ResourceKind::DescriptorSetLayout);
        ctx.resources.destroyed(ResourceKind::DescriptorSetLayout);
        ctx.resources.destroyed(ResourceKind::RenderPass);
        for _ in &self.shader_modules {
            ctx.resources.destroyed(ResourceKind::ShaderModule);
        }
    }
}

/// `include_bytes!` yields a byte slice; SPIR-V needs little-endian `u32` words.
fn spirv_words(bytes: &[u8]) -> Vec<u32> {
    if !bytes.len().is_multiple_of(4) {
        return Vec::new();
    }
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}
