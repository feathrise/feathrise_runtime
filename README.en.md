# feathrise_runtime

[简体中文](README.md) · **English**

The Bevy runtime library shared by the Feathrise (羽立) editor and Rust projects. It provides scenes, assets, rendering, terrain, water, vegetation, physics, and shared data formats. Each project supplies its own gameplay, characters, content presets, UI, and save policies through plugins.

Feathrise source code is available under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Third-party components retain their original licenses. [Third-party notices](THIRD_PARTY_NOTICES.md)

## Getting Started and Public API

The scene-based SDK uses Cargo package version `0.2.0`, Bevy 0.20.1, wgpu 30.0.1, and Rust 1.97.1 or later. [Quick Start](docs/quickstart.md) · [Using a Published Release](docs/crates-io.md)

`feathrise_plugin_api` is published separately from this repository. Its default feature set provides just the protocol and works independently of the editor and Bevy. You can develop plugins with the API package alone. [Standalone Plugin API and Examples](docs/native-plugins.md)

Start with `cargo run --locked --example minimal_project`. This example uses the public entry point to launch a custom `Plugin` and displays a rotating cube with Chinese text rendered in the default font. It runs independently of the editor and external project assets. [Minimal Project Example](docs/quickstart.md)

For local development, add the SDK as a path dependency.

```toml
[dependencies]
feathrise_runtime = { version = "0.2.0", path = "../feathrise_runtime" }
```

External projects configure patches that match their SDK version. The crates in this repository declare both `version` and `path`. Local builds use the source checkout, and published archives use the corresponding registry versions.

Configure rendering patches in the consuming project's root `Cargo.toml` or in an explicit Cargo configuration. A library's `[patch]` section applies only within its own workspace. The Bevy 0.20.1 source uses patches for `avian3d`, `bevy_anti_alias`, `bevy_render`, `bevy_core_pipeline`, `bevy_extract`, `wgpu-hal`, `dlss_wgpu`, and `wgpu-ffx`. With the crates.io SDK, use the Git revision of the standalone patches repository that matches the release.

Feathrise uses `sdk-source.toml` to select the source for scene projects. For older projects that use the registry, `version` selects the published SDK version, and `git` and `rev` select the matching patches repository and revision.

The editor can use a local SDK source checkout. [Local SDK Development](docs/crates-io.md#本地修改)

The crate root exposes `run(project, project_plugin)`, `ProjectContext`, `GameView`, `validate_saved_scene`, and the cancellable `validate_saved_scene_with_check`. The `project` module provides the project entry point, and `app` assembles the runtime. `game_plugins::SceneRuntimePlugin` sets up scenes, environment, and rendering. The scene lifecycle uses `ScenePhase`.

- `project::ProjectViewOrigin` marks the spawn reference point in the saved scene being loaded.
- `project::SceneRuntimeObserver` is the anchor for nearby physics and streaming.
- `project::PrimarySceneView` marks the main camera created by the scene lifecycle for the environment and graphics quality systems.

Project plugins implement player creation and controls such as movement, jumping, and follow cameras. The current empty project loads a saved scene and presents its scene view. Third-person and fixed top-down `GameView` values carry 3D metadata. The editor and the project runtime each use their own Bevy `World`.

The shared settings library takes runtime inputs through `RenderSettings`, `SceneDisplaySettings`, and `EnvironmentPreviewSettings`. Projects manage their own configuration and persistence. They can reuse the FPS/F3 panel and LOD status and use reports to collect their own metrics.

## Scene Project Format

The scene catalog is `scenes/catalog.scene_catalog.json` under the primary asset root. Its `scenes` field lists the scenes, and each manifest lives at `scenes/<SceneId>/scene.scene_manifest.json`. The catalog, manifests, and scene-private files use format version 2. Terrain surface data uses format version 3. Entries in a manifest's `subscenes` refer to scenes in the same catalog by `source_scene_id`.

At the project root, `startup.scene.json` uses format version 2 to store the project identity, revision, and optional `startup_scene_id`. When no startup scene is specified, the runtime uses the first published scene. An empty catalog can start an empty world.

## Cargo Features

| Feature | Purpose |
| --- | --- |
| `raytracing` (default) | Compiles in ray tracing support. Activation depends on project settings and device support. |
| `default_font` (default) | Embeds the default Noto Sans CJK font and its OFL license text. |
| `dlss`, `dlss_frame_generation`, `rtx_rendering` | Integrate DLSS with Vulkan and ray tracing. [NGX and Streamline build requirements](crates/feathrise_frame_generation/README.md) |
| `fsr`, `fsr3_upscaling`, `fsr3_frame_generation` | Enable FSR upscaling and frame generation. Frame generation requires an explicitly supplied native ZIP or a native package explicitly built from source with the bundled scripts. |
| `test_panel` | Provides debugging tools that projects can enable. |
| `physics_debug`, `initial_items` | Retain compatibility with older feature names. |
| `validation` | Enables project validation and reporting. |

Enable `feathrise_runtime/validation` in project builds that need validation reports. The root crate's `project` and `app` modules share this flag for project startup and shared runtime setup, respectively.

With `default_font` enabled, the runtime depends on the `feathrise_font` package from crates.io. That package reconstructs the original font at compile time and verifies its SHA-256 hash. The original OFL text ships with the font package and the runtime. Install `DefaultFontPlugin` after Bevy's `DefaultPlugins`. It replaces only Bevy's built-in default font and respects project-provided defaults and fonts set explicitly on widgets. The font is embedded in the executable and available independently of the source directory. Projects that supply their own font can set `default-features = false` and then enable features such as `raytracing` as needed.

## Build and Runtime Requirements

Check the default feature set from the SDK root.

```powershell
cargo check --locked -p feathrise_runtime
```

Windows builds use Rust 1.97.1 or later and MSVC. The SDK can be developed from a standalone checkout. Cargo fetches the public patches pinned in the lockfile. To work on the patches alongside the SDK, configure both the local patches and a separate development lockfile. Tests use project-independent fixtures included in the repository. [Local Development](docs/crates-io.md#本地修改) · [CI for Default and Validation Builds](docs/ci.md)

For FSR Frame Generation, set `FEATHRISE_FSR3_PACKAGE` to an existing local ZIP. The build verifies and extracts it, then links the patched DLL and static bridge library. This path works without the AMD or Vulkan SDKs, CMake, or compiling C++ for FSR. If you lack a prebuilt native package, use the scripts in the public package to build it from source with the required SDKs and native build tools. The machine running it needs a suitable Vulkan graphics driver and the Microsoft Visual C++ 2015–2022 x64 Redistributable. FSR remains experimental. Full GPU and visual validation goes beyond compilation checks. Optional DLSS builds require the DLSS SDK, LLVM/Clang, Vulkan, and MSVC, with Streamline also required for frame generation. [FSR Package Setup](crates/feathrise_fsr3_frame_generation/prebuilt/README.md) · [DLSS Build Requirements](crates/feathrise_frame_generation/README.md)

## Package and Workspace Integration Tests

Tests in each published package use only files shipped with that package. The unpublished `tests/workspace` package tests cross-crate production shader contracts through existing interfaces. Run the integration tests from the repository root.

```powershell
cargo test --locked -p feathrise_sdk_workspace_tests --lib
```

This command runs GPU-free shader integration tests by default. Specialized tests that use a real GPU are marked as ignored and can be run explicitly. The SDK authoring viewport tests write screenshots to `target/validation/author-view-window-msaa/` by default. Set `FEATHRISE_AUTHOR_VIEW_GPU_DIR` to override that path. [Workspace Integration Tests](tests/workspace/README.md)

## Repository Layout

```text
feathrise_runtime/
  Cargo.toml, Cargo.lock       Workspace and unified entry point
  src/                        Public API, project startup, and runtime setup
    project/                  Project context, startup, previews, and editor communication
    app/                      Shared scene setup, fonts, and presentation safeguards
  examples/minimal_project.rs  Runnable minimal SDK project
  crates/                     22 library crates
    feathrise_scene_core/      Core scene modules for constants, view, state, input, and residency
    feathrise_fsr3_frame_generation/
      build.rs                Cargo build entry point
      scripts/                Helpers for explicit source rebuilds
      prebuilt/               Native package lock and usage instructions
  docs/                       Minimal usage examples and CI documentation
  THIRD_PARTY_NOTICES.md       Third-party license information for the source
```

## Upgrade Compatibility

Projects upgrading from removed APIs such as `PlayerInitData`, `controls`, or `player` must migrate their source and dependencies. Projects that used `player_start.player_start.json` must now implement character creation in their own plugins. Pinned native packages retain the C ABI, filenames, and identity protocol documented for each package.
