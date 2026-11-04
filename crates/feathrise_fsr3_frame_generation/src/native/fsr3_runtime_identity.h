// 源码修订DLL无法保留AMD签名；本文件在加载边界核对编译时固定SHA，并在LoadLibrary返回前禁止文件替换。
#pragma once // 单翻译单元内只定义一次运行库文件守卫。
#include <Windows.h> // 文件句柄、共享模式和加载边界错误码。
#include <bcrypt.h> // 系统CNG实现SHA-256，不加载额外密码运行库。
#include <array> // 启动期固定64KiB流式读取缓冲。
#include <cstring> // 比较完整的固定长度十六进制哈希。
#ifndef BL5_FSR3_PATCHED_RUNTIME_SHA256 // 必须由build.rs对实际修订产物核验后固化。
#error "The patched FidelityFX DLL identity must be supplied by build.rs"
#endif // 不允许退回仅信任DLL旁边可替换的manifest。

class Bl5VerifiedRuntimeFile { // 生命周期只覆盖一次启动校验和LoadLibrary，不进入帧路径。
public: // 构造默认空句柄，任何失败都由析构统一清理。
    ~Bl5VerifiedRuntimeFile() { // 保证早退和加载失败不会泄漏文件或CNG句柄。
        if (hash_ != nullptr) BCryptDestroyHash(hash_); // 先释放依赖算法句柄的哈希对象。
        if (algorithm_ != nullptr) BCryptCloseAlgorithmProvider(algorithm_, 0); // 释放系统SHA-256算法提供器。
        if (file_ != INVALID_HANDLE_VALUE) CloseHandle(file_); // LoadLibrary完成后才解除禁止写入/删除的共享锁。
    } // 启动守卫销毁结束。
    DWORD verify(const wchar_t* path) { // 返回Win32语义错误码，任何校验失败都禁止加载。
        file_ = CreateFileW(path, GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_FLAG_SEQUENTIAL_SCAN, nullptr); // 读取期间不允许另一个进程替换已经核验的DLL。
        if (file_ == INVALID_HANDLE_VALUE) return GetLastError(); // 路径、访问和文件共享错误直接报告。
        if (BCryptOpenAlgorithmProvider(&algorithm_, BCRYPT_SHA256_ALGORITHM, nullptr, 0) < 0) return ERROR_INVALID_DATA; // 只接受系统SHA-256实现。
        if (BCryptCreateHash(algorithm_, &hash_, nullptr, 0, nullptr, 0, 0) < 0) return ERROR_INVALID_DATA; // CNG管理固定小型哈希状态，析构成对释放。
        std::array<unsigned char, 65536> bytes{}; // 不随DLL大小增长的启动期缓冲。
        for (;;) { // 只在启动边界完整覆盖DLL字节。
            DWORD count = 0; // 每轮清零实际读取长度。
            if (!ReadFile(file_, bytes.data(), static_cast<DWORD>(bytes.size()), &count, nullptr)) return GetLastError(); // 不将读取失败误判为EOF。
            if (count == 0) break; // 零字节成功读取才表示完整文件结束。
            if (BCryptHashData(hash_, bytes.data(), count, 0) < 0) return ERROR_INVALID_DATA; // 按实际读取字节增量更新。
        } // 完整DLL读取结束。
        std::array<unsigned char, 32> digest{}; // SHA-256固定256位输出。
        if (BCryptFinishHash(hash_, digest.data(), static_cast<ULONG>(digest.size()), 0) < 0) return ERROR_INVALID_DATA; // 必须成功完成整文件哈希。
        constexpr char digits[] = "0123456789ABCDEF"; // 与构建manifest的大写十六进制约定一致。
        std::array<char, 65> actual{}; // 最后一字节自动保持NUL。
        for (size_t index = 0; index < digest.size(); ++index) { // 固定32项，不涉及内容规模的帧工作。
            actual[index * 2] = digits[digest[index] >> 4]; // 编码每字节高四位。
            actual[index * 2 + 1] = digits[digest[index] & 15]; // 编码每字节低四位。
        } // 固定摘要编码结束。
        return std::strcmp(actual.data(), BL5_FSR3_PATCHED_RUNTIME_SHA256) == 0 ? ERROR_SUCCESS : ERROR_INVALID_DATA; // 只加载与应用同次构建绑定的源码修订产物。
    } // 修订DLL加载边界核验结束。
private: // 所有资源由守卫唯一拥有。
    HANDLE file_ = INVALID_HANDLE_VALUE; // 持有已核验文件直到LoadLibrary完成。
    BCRYPT_ALG_HANDLE algorithm_ = nullptr; // 系统SHA-256算法句柄。
    BCRYPT_HASH_HANDLE hash_ = nullptr; // 本次文件校验的独立哈希状态。
}; // Bl5VerifiedRuntimeFile结束。
