//! Vulkan device setup: instance, physical-device selection, logical device,
//! queue, command pool and the capability checks the renderer depends on.
//!
//! Nothing here requires hardware ray tracing: the renderer only needs a compute
//! queue, storage images, storage buffers and a swapchain.

use std::ffi::{c_char, CStr};

use ash::{vk, Entry, Instance};

use crate::error::{msg, Error, Result};

pub mod pipelines;
pub mod resources;
pub mod swapchain;

use std::sync::atomic::{AtomicI64, Ordering};

/// Categories of Vulkan objects whose lifetime this renderer manages.
///
/// Counting them makes "repeated resizing and render restarts must not leak GPU
/// resources" a *checked* property instead of a claim: the self-test records a
/// baseline, performs many resizes/restarts, and asserts the live counts return
/// to that baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum ResourceKind {
    Buffer = 0,
    Image,
    ImageView,
    DeviceMemory,
    Sampler,
    Semaphore,
    Fence,
    CommandBuffer,
    QueryPool,
    Pipeline,
    PipelineLayout,
    DescriptorSetLayout,
    DescriptorPool,
    RenderPass,
    ShaderModule,
    Framebuffer,
    Swapchain,
}

pub const RESOURCE_KIND_COUNT: usize = ResourceKind::Swapchain as usize + 1;

impl ResourceKind {
    pub const ALL: [ResourceKind; RESOURCE_KIND_COUNT] = [
        ResourceKind::Buffer,
        ResourceKind::Image,
        ResourceKind::ImageView,
        ResourceKind::DeviceMemory,
        ResourceKind::Sampler,
        ResourceKind::Semaphore,
        ResourceKind::Fence,
        ResourceKind::CommandBuffer,
        ResourceKind::QueryPool,
        ResourceKind::Pipeline,
        ResourceKind::PipelineLayout,
        ResourceKind::DescriptorSetLayout,
        ResourceKind::DescriptorPool,
        ResourceKind::RenderPass,
        ResourceKind::ShaderModule,
        ResourceKind::Framebuffer,
        ResourceKind::Swapchain,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ResourceKind::Buffer => "buffer",
            ResourceKind::Image => "image",
            ResourceKind::ImageView => "image_view",
            ResourceKind::DeviceMemory => "device_memory",
            ResourceKind::Sampler => "sampler",
            ResourceKind::Semaphore => "semaphore",
            ResourceKind::Fence => "fence",
            ResourceKind::CommandBuffer => "command_buffer",
            ResourceKind::QueryPool => "query_pool",
            ResourceKind::Pipeline => "pipeline",
            ResourceKind::PipelineLayout => "pipeline_layout",
            ResourceKind::DescriptorSetLayout => "descriptor_set_layout",
            ResourceKind::DescriptorPool => "descriptor_pool",
            ResourceKind::RenderPass => "render_pass",
            ResourceKind::ShaderModule => "shader_module",
            ResourceKind::Framebuffer => "framebuffer",
            ResourceKind::Swapchain => "swapchain",
        }
    }
}

/// Live-object counters.  Cheap atomics; used on every create/destroy.
#[derive(Debug, Default)]
pub struct ResourceCounts {
    counters: [AtomicI64; RESOURCE_KIND_COUNT],
}

impl ResourceCounts {
    pub fn created(&self, kind: ResourceKind) {
        self.counters[kind as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn destroyed(&self, kind: ResourceKind) {
        self.counters[kind as usize].fetch_sub(1, Ordering::Relaxed);
    }

    pub fn live(&self, kind: ResourceKind) -> i64 {
        self.counters[kind as usize].load(Ordering::Relaxed)
    }

    /// Live count per kind, in [`ResourceKind::ALL`] order.
    pub fn snapshot(&self) -> Vec<(ResourceKind, i64)> {
        ResourceKind::ALL
            .iter()
            .map(|kind| (*kind, self.live(*kind)))
            .collect()
    }

    /// Total number of live tracked objects.
    pub fn total(&self) -> i64 {
        self.counters
            .iter()
            .map(|counter| counter.load(Ordering::Relaxed))
            .sum()
    }

    /// Human-readable differences between two snapshots.
    pub fn diff(
        before: &[(ResourceKind, i64)],
        after: &[(ResourceKind, i64)],
    ) -> Vec<(ResourceKind, i64)> {
        before
            .iter()
            .zip(after.iter())
            .filter_map(|((kind, a), (_, b))| {
                if a == b {
                    None
                } else {
                    Some((*kind, b - a))
                }
            })
            .collect()
    }
}

/// Number of frames in flight.  The render loop is deliberately synchronous
/// (one submit + present per frame, waiting for the previous frame's guard fence
/// before recording the next), which keeps the accumulation image free of
/// cross-frame hazards without extra semaphores.
pub const FRAMES_IN_FLIGHT: usize = 1;

/// Where [`GpuInfo::driver_version`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum DriverVersionSource {
    /// Parsed from the `VK_KHR_driver_properties` `driverInfo` string.
    DriverInfo,
    /// Decoded from the packed `driverVersion` integer (no usable vendor string).
    PackedInteger,
}

/// Everything we report about the selected GPU / driver.
#[derive(Clone, Debug, serde::Serialize)]
pub struct GpuInfo {
    pub device_name: String,
    pub api_version: String,
    /// Raw `VkPhysicalDeviceProperties::driverVersion` value.
    pub driver_version_raw: u32,
    /// `driver_version_raw` in hex, for traceability.
    pub driver_version_packed_hex: String,
    /// Human-readable driver version.
    ///
    /// Taken from `VK_KHR_driver_properties` (`driverInfo`) when the vendor
    /// supplies a parseable version there, because `driverVersion` is packed
    /// with a *vendor-specific* scheme (AMD, for instance, reports `26.9.1` in
    /// `driverInfo` while its packed integer decodes to `2.0.353` under the
    /// common layout). `driver_version_source` records which was used, so the
    /// number is never presented without its provenance.
    pub driver_version: String,
    pub driver_version_source: DriverVersionSource,
    pub driver_name: String,
    pub driver_info: String,
    pub device_type: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub timestamp_period_ns: f32,
    pub timestamp_valid_bits: u32,
    pub timestamps_supported: bool,
    pub max_compute_workgroup_invocations: u32,
    pub max_compute_workgroup_size: [u32; 3],
    pub max_image_dimension_2d: u32,
    pub max_framebuffer_size: [u32; 2],
    pub non_coherent_atom_size: u64,
    pub queue_family_index: u32,
}

pub struct Context {
    pub entry: Entry,
    pub instance: Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue: vk::Queue,
    pub queue_family_index: u32,
    pub info: GpuInfo,
    pub memory_properties: vk::PhysicalDeviceMemoryProperties,
    pub command_pool: vk::CommandPool,
    pub debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    pub validation_enabled: bool,
    pub required_instance_extensions: Vec<String>,
    pub enabled_device_extensions: Vec<String>,
    /// Present only when a surface was requested.
    pub surface: Option<SurfaceInfo>,
    /// Live-object counters (see [`ResourceCounts`]).
    pub resources: ResourceCounts,
}

/// Options controlling device creation.
#[derive(Clone, Debug, Default)]
pub struct ContextOptions {
    pub validation: bool,
    /// Optional case-insensitive substring used to pick a specific GPU.
    pub gpu_name_filter: Option<String>,
    /// Require presentation support (surface + `VK_KHR_swapchain`).
    pub require_presentation: bool,
}

/// The presentation surface, created together with the instance so that the
/// physical device can be selected with full knowledge of whether it can
/// present to it.
pub struct SurfaceInfo {
    pub handle: vk::SurfaceKHR,
    pub loader: ash::khr::surface::Instance,
    pub queue_family_index: u32,
}

impl Context {
    /// Creates the instance (with optional validation layers), optionally
    /// creates the presentation surface, selects a physical device that
    /// satisfies every requirement, and creates the logical device.
    ///
    /// `instance_extensions` must contain the surface extensions for the window
    /// system (see `ash_window::enumerate_required_extensions`).
    ///
    /// `surface_creator` is invoked with the freshly created `Entry`/`Instance`
    /// so the surface exists *before* device selection; this is what allows the
    /// renderer to verify present support and `VK_KHR_swapchain` on the chosen
    /// device instead of discovering the problem later.
    pub fn new<F>(
        instance_extensions: &[*const c_char],
        options: &ContextOptions,
        surface_creator: Option<F>,
    ) -> Result<Self>
    where
        F: FnOnce(&Entry, &Instance) -> Result<vk::SurfaceKHR>,
    {
        let entry = unsafe { Entry::load() }.map_err(|e| {
            Error::Unsupported(format!(
                "could not load the Vulkan loader (vulkan-1.dll): {e}. \
                 Install a Vulkan-capable GPU driver."
            ))
        })?;

        let loader_version = unsafe { entry.try_enumerate_instance_version() }?
            .unwrap_or(vk::API_VERSION_1_0);
        if loader_version < vk::API_VERSION_1_2 {
            return Err(Error::Unsupported(format!(
                "Vulkan 1.2 is required, but the installed loader only reports {}",
                version_string(loader_version)
            )));
        }

        let available_layers = unsafe { entry.enumerate_instance_layer_properties() }?;
        let has_validation = available_layers.iter().any(|l| {
            (unsafe { CStr::from_ptr(l.layer_name.as_ptr()) }) == c"VK_LAYER_KHRONOS_validation"
        });
        if options.validation && !has_validation {
            return Err(Error::Unsupported(
                "validation layers were requested (--validation) but VK_LAYER_KHRONOS_validation \
                 is not installed; install the Vulkan SDK / validation layer package"
                    .to_string(),
            ));
        }

        let layer_names: Vec<*const c_char> = if options.validation {
            vec![c"VK_LAYER_KHRONOS_validation".as_ptr()]
        } else {
            Vec::new()
        };

        let mut extension_names: Vec<*const c_char> = instance_extensions.to_vec();
        let debug_utils_available = unsafe { entry.enumerate_instance_extension_properties(None) }?
            .iter()
            .any(|e| (unsafe { CStr::from_ptr(e.extension_name.as_ptr()) }) == ash::ext::debug_utils::NAME);
        if options.validation && debug_utils_available {
            extension_names.push(ash::ext::debug_utils::NAME.as_ptr());
        }

        let app_name = c"vk-raytracer";
        let engine_name = c"vk-raytracer";
        let app_info = vk::ApplicationInfo::default()
            .application_name(app_name)
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(engine_name)
            .engine_version(vk::make_api_version(0, 0, 1, 0))
            .api_version(vk::API_VERSION_1_2);

        let create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_layer_names(&layer_names)
            .enabled_extension_names(&extension_names);

        let instance = unsafe { entry.create_instance(&create_info, None)? };

        let debug = if options.validation && debug_utils_available {
            let debug_utils = ash::ext::debug_utils::Instance::new(&entry, &instance);
            let info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                        | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                        | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(vulkan_debug_callback));
            let messenger = unsafe { debug_utils.create_debug_utils_messenger(&info, None)? };
            Some((debug_utils, messenger))
        } else {
            None
        };

        let required_instance_extensions = instance_extensions
            .iter()
            .map(|e| unsafe { CStr::from_ptr(*e) }.to_string_lossy().into_owned())
            .collect();

        // The surface must exist before device selection so that present support
        // and the swapchain extension can be validated for the chosen device.
        let surface_handle = match surface_creator {
            Some(creator) => Some(creator(&entry, &instance)?),
            None => None,
        };
        let surface_loader = surface_handle
            .map(|_| ash::khr::surface::Instance::new(&entry, &instance));

        let (physical_device, queue_family_index, info, memory_properties) =
            pick_physical_device(&instance, options, surface_handle, surface_loader.as_ref())?;

        // Enable the device extensions this renderer uses.  Note that
        // VK_KHR_swapchain is a *device* extension: without enabling it,
        // vkGetDeviceProcAddr returns null for vkCreateSwapchainKHR.
        let available_device_extensions =
            unsafe { instance.enumerate_device_extension_properties(physical_device)? };
        let mut device_extension_names: Vec<*const c_char> = Vec::new();
        if surface_handle.is_some() {
            if !available_device_extensions.iter().any(|e| {
                (unsafe { CStr::from_ptr(e.extension_name.as_ptr()) })
                    == ash::khr::swapchain::NAME
            }) {
                return Err(Error::Unsupported(format!(
                    "the selected device does not support {}",
                    ash::khr::swapchain::NAME.to_string_lossy()
                )));
            }
            device_extension_names.push(ash::khr::swapchain::NAME.as_ptr());
        }

        let queue_priority = [1.0f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&queue_priority)];
        let device_features = vk::PhysicalDeviceFeatures::default();
        let device_create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_info)
            .enabled_extension_names(&device_extension_names)
            .enabled_features(&device_features);
        let device = unsafe { instance.create_device(physical_device, &device_create_info, None)? };
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family_index)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = unsafe { device.create_command_pool(&pool_info, None)? };

        let validation_enabled = debug.is_some();
        let enabled_device_extensions = device_extension_names
            .iter()
            .map(|e| unsafe { CStr::from_ptr(*e) }.to_string_lossy().into_owned())
            .collect();
        let surface = match (surface_handle, surface_loader) {
            (Some(handle), Some(loader)) => Some(SurfaceInfo {
                handle,
                loader,
                queue_family_index,
            }),
            _ => None,
        };
        Ok(Self {
            entry,
            instance,
            physical_device,
            device,
            queue,
            queue_family_index,
            info,
            memory_properties,
            command_pool,
            debug,
            validation_enabled,
            required_instance_extensions,
            enabled_device_extensions,
            surface,
            resources: ResourceCounts::default(),
        })
    }

    pub fn api_version(&self) -> u32 {
        unsafe { self.instance.get_physical_device_properties(self.physical_device) }.api_version
    }

    /// Finds a memory type index satisfying `type_bits` and `properties`.
    pub fn find_memory_type(
        &self,
        type_bits: u32,
        properties: vk::MemoryPropertyFlags,
    ) -> Result<u32> {
        for i in 0..self.memory_properties.memory_type_count {
            let supported = self.memory_properties.memory_types[i as usize].property_flags;
            if (type_bits & (1 << i)) != 0 && supported.contains(properties) {
                return Ok(i);
            }
        }
        msg(format!(
            "no memory type satisfies type_bits={type_bits:#x} properties={properties:?}"
        ))
    }

    /// Allocates and binds memory for `requirements`.
    pub fn allocate_memory(
        &self,
        requirements: &vk::MemoryRequirements,
        properties: vk::MemoryPropertyFlags,
    ) -> Result<vk::DeviceMemory> {
        let index = self.find_memory_type(requirements.memory_type_bits, properties)?;
        let info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(index);
        let memory = unsafe { self.device.allocate_memory(&info, None)? };
        self.resources.created(ResourceKind::DeviceMemory);
        Ok(memory)
    }

    pub fn allocate_command_buffer(&self) -> Result<vk::CommandBuffer> {
        let info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let buffers = unsafe { self.device.allocate_command_buffers(&info)? };
        for _ in 0..buffers.len() {
            self.resources.created(ResourceKind::CommandBuffer);
        }
        Ok(buffers[0])
    }

    /// Records, submits and waits for a single one-shot command buffer.
    /// Creates a fence and accounts for it (use [`Context::destroy_fence`]).
    pub fn create_fence(&self, signaled: bool) -> Result<vk::Fence> {
        let flags = if signaled {
            vk::FenceCreateFlags::SIGNALED
        } else {
            vk::FenceCreateFlags::empty()
        };
        let fence = unsafe {
            self.device
                .create_fence(&vk::FenceCreateInfo::default().flags(flags), None)?
        };
        self.resources.created(ResourceKind::Fence);
        Ok(fence)
    }

    pub fn destroy_fence(&self, fence: vk::Fence) {
        unsafe { self.device.destroy_fence(fence, None) };
        self.resources.destroyed(ResourceKind::Fence);
    }

    /// Creates a binary semaphore and accounts for it.
    pub fn create_semaphore(&self) -> Result<vk::Semaphore> {
        let semaphore = unsafe {
            self.device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?
        };
        self.resources.created(ResourceKind::Semaphore);
        Ok(semaphore)
    }

    pub fn destroy_semaphore(&self, semaphore: vk::Semaphore) {
        unsafe { self.device.destroy_semaphore(semaphore, None) };
        self.resources.destroyed(ResourceKind::Semaphore);
    }

    pub fn free_command_buffer(&self, command_buffer: vk::CommandBuffer) {
        unsafe {
            self.device
                .free_command_buffers(self.command_pool, &[command_buffer]);
        }
        self.resources.destroyed(ResourceKind::CommandBuffer);
    }

    pub fn one_shot<F>(&self, record: F) -> Result<()>
    where
        F: FnOnce(vk::CommandBuffer),
    {
        let command_buffer = self.allocate_command_buffer()?;
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        let fence = self.create_fence(false)?;
        let result = (|| -> Result<()> {
            unsafe {
                self.device.begin_command_buffer(command_buffer, &begin)?;
                record(command_buffer);
                self.device.end_command_buffer(command_buffer)?;
            }
            let command_buffers = [command_buffer];
            let submit = vk::SubmitInfo::default().command_buffers(&command_buffers);
            unsafe {
                self.device.queue_submit(self.queue, &[submit], fence)?;
                self.device.wait_for_fences(&[fence], true, u64::MAX)?;
            }
            Ok(())
        })();
        self.destroy_fence(fence);
        self.free_command_buffer(command_buffer);
        result
    }

    pub fn wait_idle(&self) -> Result<()> {
        Ok(unsafe { self.device.device_wait_idle() }?)
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            if let Some((debug_utils, messenger)) = self.debug.take() {
                debug_utils.destroy_debug_utils_messenger(messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

unsafe extern "system" fn vulkan_debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    kind: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _user_data: *mut std::ffi::c_void,
) -> vk::Bool32 {
    let data = unsafe { &*data };
    let message = if data.p_message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(data.p_message) }
            .to_string_lossy()
            .into_owned()
    };
    eprintln!("[vulkan {severity:?} {kind:?}] {message}");
    vk::FALSE
}

fn pick_physical_device(
    instance: &Instance,
    options: &ContextOptions,
    surface: Option<vk::SurfaceKHR>,
    surface_loader: Option<&ash::khr::surface::Instance>,
) -> Result<(
    vk::PhysicalDevice,
    u32,
    GpuInfo,
    vk::PhysicalDeviceMemoryProperties,
)> {
    let devices = unsafe { instance.enumerate_physical_devices()? };
    if devices.is_empty() {
        return Err(Error::Unsupported(
            "no Vulkan-capable physical device was found".to_string(),
        ));
    }

    let filter = options
        .gpu_name_filter
        .as_ref()
        .map(|f| f.to_ascii_lowercase());

    let mut candidates: Vec<(i32, vk::PhysicalDevice, u32, vk::PhysicalDeviceProperties)> =
        Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut enumerated = 0usize;
    let mut filtered_out = 0usize;

    for device in devices {
        enumerated += 1;
        let properties = unsafe { instance.get_physical_device_properties(device) };
        let name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();

        if let Some(filter) = &filter {
            if !name.to_ascii_lowercase().contains(filter) {
                filtered_out += 1;
                continue;
            }
        }

        let queue_properties =
            unsafe { instance.get_physical_device_queue_family_properties(device) };
        let Some(family) = queue_properties.iter().position(|q| {
            q.queue_flags.contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
        }) else {
            notes.push(format!("{name}: no graphics+compute queue family"));
            continue;
        };

        // Presentation support must be verified per device/queue family.
        if let (Some(surface), Some(loader)) = (surface, surface_loader) {
            let present_supported = unsafe {
                loader.get_physical_device_surface_support(device, family as u32, surface)
            }
            .unwrap_or(false);
            if !present_supported {
                notes.push(format!(
                    "{name}: queue family {family} cannot present to the window surface"
                ));
                continue;
            }
        }

        // Capability checks that would make rendering impossible.
        let mut problems = Vec::new();
        check_format(
            instance,
            device,
            vk::Format::R32G32B32A32_SFLOAT,
            vk::FormatFeatureFlags::STORAGE_IMAGE
                | vk::FormatFeatureFlags::SAMPLED_IMAGE
                | vk::FormatFeatureFlags::TRANSFER_SRC,
            "R32G32B32A32_SFLOAT storage/sampled/transfer",
            &mut problems,
        );
        check_format(
            instance,
            device,
            vk::Format::R8G8B8A8_UNORM,
            vk::FormatFeatureFlags::SAMPLED_IMAGE,
            "R8G8B8A8_UNORM sampled (font atlas)",
            &mut problems,
        );
        let limits = properties.limits;
        if limits.max_compute_work_group_invocations < 64
            || limits.max_compute_work_group_size[0] < 8
            || limits.max_compute_work_group_size[1] < 8
        {
            problems.push(format!(
                "compute work group limits too small (invocations={}, size={:?})",
                limits.max_compute_work_group_invocations, limits.max_compute_work_group_size
            ));
        }
        if limits.max_image_dimension2_d < 4096 {
            problems.push(format!(
                "maxImageDimension2D={} is below the required 4096",
                limits.max_image_dimension2_d
            ));
        }
        if limits.max_descriptor_set_storage_buffers < 4
            || limits.max_descriptor_set_storage_images < 1
            || limits.max_descriptor_set_uniform_buffers < 1
            || limits.max_per_stage_descriptor_storage_buffers < 4
            || limits.max_per_stage_descriptor_storage_images < 1
        {
            problems.push("insufficient descriptor counts for the ray tracing pipeline".to_string());
        }
        if limits.max_push_constants_size < 16 {
            problems.push("maxPushConstantsSize < 16 bytes".to_string());
        }
        if limits.max_storage_buffer_range < 4096 {
            problems.push("maxStorageBufferRange too small".to_string());
        }
        if !problems.is_empty() {
            notes.push(format!("{name}: {}", problems.join("; ")));
            continue;
        }

        // Score: prefer the requested GPU, then discrete > integrated > other.
        let mut score = 0i32;
        if filter.is_some() {
            score += 1000;
        }
        if name.to_ascii_lowercase().contains("5700 xt") {
            score += 100;
        }
        score += match properties.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 50,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 20,
            vk::PhysicalDeviceType::VIRTUAL_GPU => 10,
            _ => 0,
        };
        candidates.push((score, device, family as u32, properties));
    }

    candidates.sort_by_key(|(score, _, _, _)| std::cmp::Reverse(*score));
    let Some((_, physical_device, queue_family_index, properties)) = candidates.into_iter().next()
    else {
        let mut reasons = Vec::new();
        if filtered_out > 0 {
            reasons.push(format!(
                "{filtered_out} device(s) excluded by the --gpu filter {:?}",
                options.gpu_name_filter.as_deref().unwrap_or("")
            ));
        }
        reasons.extend(notes);
        return Err(Error::Unsupported(format!(
            "no suitable Vulkan device found ({enumerated} physical device(s) enumerated{}){}",
            if filtered_out > 0 {
                format!(", {filtered_out} filtered out")
            } else {
                String::new()
            },
            if reasons.is_empty() {
                String::new()
            } else {
                format!(": {}", reasons.join(" | "))
            }
        )));
    };

    let mut driver_properties = vk::PhysicalDeviceDriverProperties::default();
    let mut properties2 = vk::PhysicalDeviceProperties2::default().push_next(&mut driver_properties);
    unsafe { instance.get_physical_device_properties2(physical_device, &mut properties2) };

    let queue_properties = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    let timestamp_valid_bits = queue_properties[queue_family_index as usize].timestamp_valid_bits;
    let memory_properties =
        unsafe { instance.get_physical_device_memory_properties(physical_device) };

    let device_name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let driver_name = unsafe { CStr::from_ptr(driver_properties.driver_name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let driver_info = unsafe { CStr::from_ptr(driver_properties.driver_info.as_ptr()) }
        .to_string_lossy()
        .into_owned();

    let timestamps_supported = timestamp_valid_bits > 0 && properties.limits.timestamp_period > 0.0;
    let info = GpuInfo {
        device_name,
        api_version: version_string(properties.api_version),
        driver_version_raw: properties.driver_version,
        driver_version_packed_hex: format!("0x{:08x}", properties.driver_version),
        driver_version: driver_version_from(&driver_info, properties.driver_version).0,
        driver_version_source: driver_version_from(&driver_info, properties.driver_version).1,
        driver_name,
        driver_info,
        device_type: format!("{:?}", properties.device_type),
        vendor_id: properties.vendor_id,
        device_id: properties.device_id,
        timestamp_period_ns: properties.limits.timestamp_period,
        timestamp_valid_bits,
        timestamps_supported,
        max_compute_workgroup_invocations: properties.limits.max_compute_work_group_invocations,
        max_compute_workgroup_size: properties.limits.max_compute_work_group_size,
        max_image_dimension_2d: properties.limits.max_image_dimension2_d,
        max_framebuffer_size: [
            properties.limits.max_framebuffer_width,
            properties.limits.max_framebuffer_height,
        ],
        non_coherent_atom_size: properties.limits.non_coherent_atom_size,
        queue_family_index,
    };

    Ok((physical_device, queue_family_index, info, memory_properties))
}

fn check_format(
    instance: &Instance,
    device: vk::PhysicalDevice,
    format: vk::Format,
    required: vk::FormatFeatureFlags,
    label: &str,
    problems: &mut Vec<String>,
) {
    let properties = unsafe { instance.get_physical_device_format_properties(device, format) };
    if !properties.optimal_tiling_features.contains(required) {
        problems.push(format!(
            "{label}: optimal tiling features {:?} lack {required:?}",
            properties.optimal_tiling_features
        ));
    }
}

/// Extracts the leading version token from a `driverInfo` string, e.g.
/// `"26.9.1 (AMD proprietary shader compiler)"` -> `"26.9.1"`.
fn leading_version_token(driver_info: &str) -> Option<String> {
    let token = driver_info.split_whitespace().next()?;
    let token = token.trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
    let mut parts = token.split('.');
    let first = parts.next()?;
    if first.is_empty() || !first.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if parts.any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    Some(token.to_string())
}

/// Decodes the packed `driverVersion` using the layout the Vulkan specification
/// describes for the common case: `(major << 22) | (minor << 12) | patch`.
///
/// This layout is *not* universal - AMD reports its real version through
/// `driverInfo` instead - so the result is only used when no vendor string is
/// available, and [`GpuInfo::driver_version_source`] records the provenance.
pub fn decode_packed_driver_version(raw: u32) -> String {
    let major = (raw >> 22) & 0x3ff;
    let minor = (raw >> 12) & 0x3ff;
    let patch = raw & 0xfff;
    if major == 0 && minor == 0 && patch == 0 {
        format!("0x{raw:08x}")
    } else {
        format!("{major}.{minor}.{patch}")
    }
}

/// Chooses the reported driver version and records where it came from.
fn driver_version_from(driver_info: &str, raw: u32) -> (String, DriverVersionSource) {
    match leading_version_token(driver_info) {
        Some(version) => (version, DriverVersionSource::DriverInfo),
        None => (decode_packed_driver_version(raw), DriverVersionSource::PackedInteger),
    }
}

pub fn version_string(version: u32) -> String {
    format!(
        "{}.{}.{}",
        vk::api_version_major(version),
        vk::api_version_minor(version),
        vk::api_version_patch(version)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_info_version_wins_over_the_packed_integer() {
        // The real values from an RX 5700 XT on Adrenalin 26.9.1: the packed
        // integer decodes to 2.0.353 under the common layout, but the vendor
        // string is authoritative.
        let (version, source) = driver_version_from("26.9.1 (AMD proprietary shader compiler)", 0x0080_0161);
        assert_eq!(version, "26.9.1");
        assert_eq!(source, DriverVersionSource::DriverInfo);
    }

    #[test]
    fn packed_integer_is_the_fallback_with_recorded_provenance() {
        let (version, source) = driver_version_from("", 0x0080_0161);
        assert_eq!(version, "2.0.353");
        assert_eq!(source, DriverVersionSource::PackedInteger);
        assert_eq!(decode_packed_driver_version(0), "0x00000000");
    }

    #[test]
    fn leading_version_token_parsing() {
        assert_eq!(leading_version_token("26.9.1").as_deref(), Some("26.9.1"));
        assert_eq!(
            leading_version_token("535.104.05 (NVIDIA)").as_deref(),
            Some("535.104.05")
        );
        assert_eq!(leading_version_token("Mesa 24.0.1"), None);
        assert_eq!(leading_version_token(""), None);
        assert_eq!(leading_version_token("26."), None);
        assert_eq!(leading_version_token("26.x"), None);
    }
}
