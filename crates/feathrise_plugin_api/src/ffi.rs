//! 版本化C函数表和Rust导出辅助；响应仅在回调期间借用，分配和释放始终同侧。
use crate::{MAX_MESSAGE_BYTES, Request, SessionConfig, Snapshot};
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
};

// 场景领域请求和回执的 JSON 字段随场景格式换代，由 ABI 主版本阻止加载旧插件。
pub const ABI_MAJOR: u32 = 2;
pub const ABI_MINOR: u32 = 0;
pub const ENTRY_SYMBOL: &[u8] = b"feathrise_plugin_api_v2\0";
pub const OK: i32 = 0;
pub const INVALID: i32 = -1;
pub const FAULT: i32 = -2;
pub type WriteResponse = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginApi {
    pub abi_major: u32,
    pub abi_minor: u32,
    pub struct_size: u32,
    pub reserved: u32,
    pub create:
        unsafe extern "C" fn(*const u8, usize, *mut *mut c_void, *mut c_void, WriteResponse) -> i32,
    pub invoke:
        unsafe extern "C" fn(*mut c_void, *const u8, usize, *mut c_void, WriteResponse) -> i32,
    pub destroy: unsafe extern "C" fn(*mut c_void),
}
pub type PluginEntry = unsafe extern "C" fn() -> *const PluginApi;

/// 该trait只在插件自己的动态库中单态化，永远不作为trait对象跨ABI传递。
pub trait NativeTool: Sized {
    fn create(config: SessionConfig) -> Result<Self, String>;
    fn request(&mut self, request: Request) -> Snapshot;
}
struct Instance<T> {
    tool: T,
    faulted: bool,
}

unsafe fn decode<T: serde::de::DeserializeOwned>(ptr: *const u8, len: usize) -> Result<T, String> {
    if ptr.is_null() || len == 0 || len > MAX_MESSAGE_BYTES {
        return Err("插件请求字节数无效".into());
    }
    serde_json::from_slice(unsafe { std::slice::from_raw_parts(ptr, len) })
        .map_err(|e| e.to_string())
}
unsafe fn reply(context: *mut c_void, writer: WriteResponse, bytes: &[u8]) -> i32 {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return INVALID;
    }
    unsafe { writer(context, bytes.as_ptr(), bytes.len()) }
}

/// 调用者保证指针可读及输出句柄可写；创建失败的消息是UTF-8文本，不假造成功快照。
pub unsafe extern "C" fn create<T: NativeTool>(
    ptr: *const u8,
    len: usize,
    out: *mut *mut c_void,
    context: *mut c_void,
    writer: WriteResponse,
) -> i32 {
    if out.is_null() {
        return INVALID;
    }
    unsafe {
        *out = std::ptr::null_mut();
    }
    match catch_unwind(AssertUnwindSafe(|| -> Result<T, String> {
        T::create(unsafe { decode(ptr, len) }?)
    })) {
        Ok(Ok(tool)) => {
            unsafe {
                *out = Box::into_raw(Box::new(Instance {
                    tool,
                    faulted: false,
                }))
                .cast();
            }
            OK
        }
        Ok(Err(error)) => {
            unsafe {
                reply(context, writer, error.as_bytes());
            }
            INVALID
        }
        Err(_) => FAULT,
    }
}
/// 同一会话只能由其拥有线程串行调用。panic后禁止重入已部分修改的插件会话。
pub unsafe extern "C" fn invoke<T: NativeTool>(
    instance: *mut c_void,
    ptr: *const u8,
    len: usize,
    context: *mut c_void,
    writer: WriteResponse,
) -> i32 {
    if instance.is_null() {
        return INVALID;
    }
    let instance = unsafe { &mut *instance.cast::<Instance<T>>() };
    if instance.faulted {
        return FAULT;
    }
    match catch_unwind(AssertUnwindSafe(|| -> Result<Vec<u8>, String> {
        let request = unsafe { decode(ptr, len) }?;
        serde_json::to_vec(&instance.tool.request(request)).map_err(|e| e.to_string())
    })) {
        Ok(Ok(bytes)) => unsafe { reply(context, writer, &bytes) },
        Ok(Err(_)) => INVALID,
        Err(_) => {
            instance.faulted = true;
            FAULT
        }
    }
}
/// 仅调用侧已停止请求后销毁；宿主仍须把动态库保持加载到进程退出。
pub unsafe extern "C" fn destroy<T: NativeTool>(instance: *mut c_void) {
    if !instance.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            drop(unsafe { Box::from_raw(instance.cast::<Instance<T>>()) });
        }));
    }
}

#[macro_export]
macro_rules! export_plugin {
    ($tool:ty, embedded) => {
        include!(concat!(env!("OUT_DIR"), "/feathrise_plugin_metadata.rs"));
        // 节数据由getter实际引用，防止链接器把只有#[used]的孤立静态量裁剪。
        #[used]
        #[cfg_attr(target_os = "macos", unsafe(link_section = "__DATA,__frmeta"))]
        #[cfg_attr(not(target_os = "macos"), unsafe(link_section = ".frmeta"))]
        static FEATHRISE_PLUGIN_METADATA: [u8; FEATHRISE_METADATA_BYTES.len()] = FEATHRISE_METADATA_BYTES;

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn feathrise_plugin_metadata_v1(
            context: *mut std::ffi::c_void,
            writer: $crate::WriteResponse,
        ) -> i32 {
            unsafe { writer(context, FEATHRISE_PLUGIN_METADATA.as_ptr(), FEATHRISE_PLUGIN_METADATA.len()) }
        }
        $crate::export_plugin!($tool);
    };
    ($tool:ty) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn feathrise_plugin_api_v2() -> *const $crate::PluginApi {
            static API: $crate::PluginApi = $crate::PluginApi {
                abi_major: $crate::ABI_MAJOR,
                abi_minor: $crate::ABI_MINOR,
                struct_size: std::mem::size_of::<$crate::PluginApi>() as u32,
                reserved: 0,
                create: $crate::create::<$tool>,
                invoke: $crate::invoke::<$tool>,
                destroy: $crate::destroy::<$tool>,
            };
            &API
        }
    };
}
