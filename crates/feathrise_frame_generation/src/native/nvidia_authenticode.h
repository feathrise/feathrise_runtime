// 验证非Streamline双签名依赖的标准Authenticode可信链及实际已验证NVIDIA签名者。
#pragma once // 仅由原生桥引入一次。
#include <Windows.h> // 系统目录安全加载与错误码。
#include <Softpub.h> // WinTrust代码签名策略与provider结构。
#include <wincrypt.h> // 已验证证书的名称读取。
#include <wintrust.h> // 可信链、时间戳与状态释放接口。
#include <cwchar> // 对实际签名者名称进行精确比较。
#include <iterator> // std::size对固定名称缓冲计算准确容量。

namespace bl5_security { // 不改变NVIDIA官方sl_security实现。
inline bool verify_nvidia_authenticode(const wchar_t* absolute_path) { // 调用者传入固定白名单绝对路径，哈希另由Rust验证。
    const HMODULE trust = LoadLibraryExW(L"wintrust.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32); // 不从当前目录加载验证器。
    const HMODULE crypto = LoadLibraryExW(L"crypt32.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32); // 名称读取同样只来自系统库。
    if (trust == nullptr || crypto == nullptr) { // 任何系统验证组件缺失时拒绝加载。
        if (trust != nullptr) { FreeLibrary(trust); } // 释放本次持有的引用。
        if (crypto != nullptr) { FreeLibrary(crypto); } // 不残留半加载资源。
        return false; // 不以读证书替代可信链验证。
    } // 系统库检查结束。
    using Verify = LONG (WINAPI*)(HWND, GUID*, LPVOID); // WinVerifyTrust ABI。
    using Provider = CRYPT_PROVIDER_DATA* (WINAPI*)(HANDLE); // 读取同次验证结果，不能重新解析未验证证书。
    using Signer = CRYPT_PROVIDER_SGNR* (WINAPI*)(CRYPT_PROVIDER_DATA*, DWORD, BOOL, DWORD); // 选择主签名者，排除时间戳签名者。
    using Name = DWORD (WINAPI*)(PCCERT_CONTEXT, DWORD, DWORD, void*, LPWSTR, DWORD); // 获取实际签名证书名称。
    const auto verify = reinterpret_cast<Verify>(GetProcAddress(trust, "WinVerifyTrust")); // 动态解析不增加加载器依赖。
    const auto provider = reinterpret_cast<Provider>(GetProcAddress(trust, "WTHelperProvDataFromStateData")); // 成功状态的provider。
    const auto signer = reinterpret_cast<Signer>(GetProcAddress(trust, "WTHelperGetProvSignerFromChain")); // 成功状态的签名链。
    const auto name = reinterpret_cast<Name>(GetProcAddress(crypto, "CertGetNameStringW")); // 不读取PE版本信息冒充签名。
    bool valid = false; // 默认拒绝，必须同时具备可信链与NVIDIA签名者。
    if (verify != nullptr && provider != nullptr && signer != nullptr && name != nullptr) { // 任一入口缺失不能降级。
        WINTRUST_FILE_INFO file{}; // 精确目标文件。
        file.cbStruct = sizeof(file); // Win32版本字段。
        file.pcwszFilePath = absolute_path; // 不执行路径搜索。
        WINTRUST_DATA data{}; // 使用标准代码签名策略，不绕过证书过期或时间戳。
        data.cbStruct = sizeof(data); // WinTrust版本字段。
        data.dwUIChoice = WTD_UI_NONE; // 初始化不得弹交互认证窗口。
        data.fdwRevocationChecks = WTD_REVOKE_NONE; // 与NVIDIA官方helper相同撤销策略；可信链和时间戳仍由WinTrust验证。
        data.dwUnionChoice = WTD_CHOICE_FILE; // 验证完整PE签名，而不是单独信任证书CN。
        data.pFile = &file; // 绑定本次待加载文件。
        data.dwStateAction = WTD_STATEACTION_VERIFY; // 保留可信验证结果供签名者检查。
        GUID policy = WINTRUST_ACTION_GENERIC_VERIFY_V2; // 验证文件摘要、代码签名EKU、可信根与有效时间戳。
        const LONG result = verify(nullptr, &policy, &data); // 只有ERROR_SUCCESS才认为链和PE签名有效。
        if (result == ERROR_SUCCESS && data.hWVTStateData != nullptr) { // 不接受不可信、过期且无有效时间戳或被修改文件。
            CRYPT_PROVIDER_DATA* verified = provider(data.hWVTStateData); // 读取同一成功验证的证据。
            CRYPT_PROVIDER_SGNR* primary = verified != nullptr ? signer(verified, 0, FALSE, 0) : nullptr; // FALSE确保不是时间戳机构。
            if (primary != nullptr && primary->csCertChain > 0 && primary->pasCertChain != nullptr && primary->pasCertChain[0].pCert != nullptr) { // 只接受实际叶签名证书。
                wchar_t publisher[128]{}; // 固定长度名称，不接受截断。
                const DWORD length = name(primary->pasCertChain[0].pCert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, nullptr, publisher, static_cast<DWORD>(std::size(publisher))); // 提取已验证签名者，不看任意附带证书。
                valid = length > 1 && length < std::size(publisher) && std::wcscmp(publisher, L"NVIDIA Corporation") == 0; // 明确信任厂商身份，固定哈希进一步锁定发行文件。
            } // 签名者检查结束。
        } // 可信链检查结束。
        data.dwStateAction = WTD_STATEACTION_CLOSE; // 即使验证失败也释放WinTrust证据对象。
        verify(nullptr, &policy, &data); // 清理调用结果不覆盖本次真实性判断。
    } // 全部验证入口结束。
    FreeLibrary(crypto); // 本次启动校验后释放系统库引用。
    FreeLibrary(trust); // provider对象已经关闭，不遗留借用。
    return valid; // 不要求非SL模块携带Streamline专属二级签名。
} // 标准NVIDIA Authenticode验证结束。
} // 安全辅助命名空间结束。
