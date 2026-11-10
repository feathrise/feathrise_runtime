# 公共内容测试夹具

这些文件是独立合成的测试输入。

`vegetation_presets.json`保留测试使用的七个稳定技术ID，声明四种四档Blade和三种四档Mesh。默认工程目录为空。`gltf/fixture_vegetation.gltf`只有内嵌152字节数据的四面体和中性材质，供正式GLTF加载、异步失败恢复、近场碰撞和目录换代回归使用。

此处的合成几何与目录用于验证公共协议和执行链。

材质引用使用 Bevy 0.19 的 `#Material0/std` 标准 PBR 子资产；`#Material0` 本身是 glTF 材质描述，不能作为 `StandardMaterial` 加载。场景回归使用真实 `PbrPlugin` 转换，并核对转换后的颜色与粗糙度。
