//! 构建、启动与编辑器交付共用的七项生产 DLL 固定身份，禁止版本漂移。
pub const RUNTIME_FILES: [(&str, &str); 7] = [
    // 首期2x只分发DLSS-G、Reflex、PCL及其直接运行依赖。
    (
        "sl.interposer.dll",
        "2A79DB6857AE8C75BBD871A9489C48BC6A39F7FCC88B9B02AFD53D0376CBEC66",
    ), // Vulkan入口与Present代理。
    (
        "sl.common.dll",
        "C57930EF5A8A3FE9BE85EFDF71A61D8107C1148E8A6AED456464547128F7F4AE",
    ), // Streamline公共插件。
    (
        "sl.dlss_g.dll",
        "1FEC3F8FDFC59D78C4445C276C1A0FB798BF251985F348597DC2B44D0C995E52",
    ), // DLSS Frame Generation插件。
    (
        "sl.reflex.dll",
        "7E6E4CCC4B561BD449FB0DA90709D9B96B08C3F6F4697362CAAA359E72A58A67",
    ), // Reflex低延迟插件。
    (
        "sl.pcl.dll",
        "699AB461E64E95189A7FE6A21C79AD237CF56B60EA748CB6C840CD5431BA91D1",
    ), // PC Latency标记插件。
    (
        "NvLowLatencyVk.dll",
        "2A77DC3E1C724B7EEA5755BE0AE7423752E79A2459FAE72181A9F00E3507E5D6",
    ), // Vulkan Reflex驱动桥。
    (
        "nvngx_dlssg.dll",
        "135EAF0733C1E37381A8C28ABCF7A862404A54132B81787C04E35D09EFC5E36F",
    ), // 官方生产FG NGX运行库。
]; // 精确运行时白名单结束，其他Streamline插件不会被复制或加载。
