//! 在Vulkan设备创建前协商privateData；Streamline用它保存和查找交换链的内部关联对象。
use ash::vk; // 直接使用与wgpu-hal相同版本的Vulkan结构。
use bevy::{prelude::*, render::renderer::raw_vulkan_init::RawVulkanInitSettings};
use std::sync::Mutex; // 回调可以被不同适配器调用，节点仓库需串行追加。 // 复用Bevy原生设备扩展入口。

struct FeatureNode(vk::PhysicalDevicePrivateDataFeatures<'static>); // Box固定地址，直到初始化设置释放才销毁。
unsafe impl Send for FeatureNode {} // 节点只在回调持锁时写入；pNext只供紧接回调的vkCreateDevice读取，仓库绝不解引用旧链。

pub(super) fn register(app: &mut App) {
    // 仅在Streamline引导成功且尚未创建设备时调用。
    let nodes = Mutex::new(Vec::<Box<FeatureNode>>::new()); // 每次设备创建保留独立节点，重试不会覆盖仍被读取的内存。
    let mut settings = app
        .world_mut()
        .get_resource_or_init::<RawVulkanInitSettings>(); // 与SR等已有回调叠加，不覆盖它们。
    unsafe {
        // 安全条件：先查询支持，只增加能力；节点所有者比设备创建调用活得更久。
        settings.add_create_device_callback(move |args, adapter, _additional| {
            // Bevy在实际vkCreateDevice前执行此闭包。
            let instance = adapter.shared_instance().raw_instance(); // 使用同一SL代理实例查询适配器。
            let physical_device = adapter.raw_physical_device(); // 不猜测或重新选择物理设备。
            let properties = instance.get_physical_device_properties(physical_device); // 固定大小能力查询，仅在启动执行。
            let core_supported = properties.api_version >= vk::API_VERSION_1_3
                && adapter.shared_instance().instance_api_version() >= vk::API_VERSION_1_3; // privateData在Vulkan1.3提升为核心。
            let extension_supported = instance
                .enumerate_device_extension_properties(physical_device)
                .is_ok_and(|extensions| {
                    extensions.iter().any(|extension| {
                        extension.extension_name_as_c_str() == Ok(ash::ext::private_data::NAME)
                    })
                }); // 较旧核心版本必须明确支持扩展。
            if !core_supported && !extension_supported {
                // 不向驱动声明未提供的特性结构。
                warn!("DLSS-G设备未提供Vulkan privateData，SDK交换链关联不可用"); // 给能力不足留下精确原因。
                return; // 保留原有设备配置和普通渲染能力。
            } // API/扩展资格检查结束。
            let mut support = vk::PhysicalDevicePrivateDataFeatures::default(); // 查询结构不声明任何能力。
            let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut support); // 查询独立临时链，不改设备配置。
            instance.get_physical_device_features2(physical_device, &mut features); // Vulkan1.1查询入口，当前Bevy Vulkan最低路径已提供。
            if support.private_data != vk::TRUE {
                // 扩展名字存在不代表对应可选能力可启用。
                warn!("DLSS-G设备privateData功能位不受支持，SDK交换链关联不可用"); // 明确区分扩展缺失和功能位关闭。
                return; // 不触发FEATURE_NOT_PRESENT设备创建失败。
            } // 功能位查询结束。
            if !core_supported && !args.extensions.contains(&ash::ext::private_data::NAME) {
                args.extensions.push(ash::ext::private_data::NAME);
            } // 扩展路径只追加一次，核心路径无需扩展名。
            let mut current = args
                .create_info
                .p_next
                .cast::<vk::BaseOutStructure<'_>>()
                .cast_mut(); // 审查已有链，避免重复同类结构。
            while !current.is_null() {
                // 初始化链长度有界，且由已有合法回调构建。
                if (*current).s_type == vk::StructureType::PHYSICAL_DEVICE_PRIVATE_DATA_FEATURES {
                    // 已有单项结构时直接启用它。
                    (*current.cast::<vk::PhysicalDevicePrivateDataFeatures<'_>>()).private_data =
                        vk::TRUE; // 不修改其他字段或链连接。
                    return; // 不再追加重复sType。
                } // 单项结构处理结束。
                if (*current).s_type == vk::StructureType::PHYSICAL_DEVICE_VULKAN_1_3_FEATURES {
                    // 聚合结构与单项结构也不得同时存在。
                    (*current.cast::<vk::PhysicalDeviceVulkan13Features<'_>>()).private_data =
                        vk::TRUE; // 只开启已验证能力，保留其他功能位。
                    return; // 使用现有聚合节点。
                } // 聚合结构处理结束。
                current = (*current).p_next; // 仅沿已有有效链前进。
            } // 现有链审查结束。
            let mut node = Box::new(FeatureNode(
                vk::PhysicalDevicePrivateDataFeatures::default().private_data(true),
            )); // 堆地址不随仓库扩容变化。
            node.0.p_next = args.create_info.p_next.cast_mut(); // 保留其他回调已经登记的完整链。
            let pointer = std::ptr::from_ref(&node.0).cast(); // 指针在所有者转入仓库后仍指向同一Box分配。
            nodes
                .lock()
                .expect("privateData feature storage poisoned")
                .push(node); // 回调返回后节点继续存活，避免栈上pNext悬空。
            args.create_info.p_next = pointer; // 只在节点已获得稳定所有权后公开给HAL。
            info!("DLSS-G Vulkan privateData能力已协商启用"); // 验证日志可区分支持与实际设备创建请求。
        }); // 注册结束；闭包仓库跟随RawVulkanInitSettings生命周期。
    } // 不安全原生初始化边界结束。
} // register结束。
