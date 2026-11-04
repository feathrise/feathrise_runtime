# SDK 仓库级集成测试

本包只属于完整 SDK 源码仓库，`publish = false`，不参与任何生产依赖。它集中核对植被、风、地表、Clipmap、水面之间的真实 Shader 接线，不复制生产 WGSL，也不为测试增加运行时公开接口。

补丁仓与 SDK 同级检出后，在 SDK 根执行：

```powershell
cargo test --locked -p feathrise_sdk_workspace_tests --lib
```

测试复用 SDK 工作区的锁文件、profile 和既有 `target/`。默认执行四项无 GPU 的生产 Shader 契约；地表根图／实例预热仍为显式忽略的真实 GPU 专项，单独执行需要相应显卡环境，不把普通测试通过计为该专项通过。

每个发布子包自己的测试只能读取该包内的输入。发包前应核对实际归档的文件集合，包括 `cfg(test)` 与默认忽略的测试源码。
