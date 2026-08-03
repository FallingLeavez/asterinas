#RECORD 1 遇到了问题:`cargo-component` 固定在 `nightly-2023-02-05`，该版本 Cargo 不支持其 Rust 2024 manifest；工具因此长期无法构建。做了改动:将 `kernel/libs/comp-sys/cargo-component/rust-toolchain.toml` 对齐到仓库的 `nightly-2026-07-21`，并保留 `rust-src`、`rustc-dev`、`llvm-tools-preview`。其他:该工具是使用 `rustc_private`/MIR 检查 `#[controlled]` 跨 crate 访问是否符合 `Components.toml` 白名单的 Cargo 子命令。

#RECORD 2 遇到了问题:PR #3637 与当前 `pr-3674` 分叉，且包含 `kernel/core` 重组；根目录执行 `cargo test` 会测试整个 workspace 并在无关的 `ostd` 构建处失败。做了改动:合并 PR #3637 到当前分支，合并提交为 `ea4a78c62`。其他:应在 `kernel/libs/comp-sys/cargo-component` 中运行 `cargo test`，或指定该目录的 `--manifest-path`。

#RECORD 3 遇到了问题:在 `cargo-component` 目录执行测试时，Cargo 报“current package believes it's in a workspace when it's not”。做了改动:在 `cargo-component/Cargo.toml` 声明独立 `[workspace]`，将 `analysis` 作为成员，并删除 `analysis/Cargo.toml` 中的嵌套 `[workspace]`。其他:下一步重新运行 `cargo test`，根据出现的 `rustc_private`/MIR API 编译错误逐项迁移分析器与 driver；`controlled` 仍位于 `kernel/libs/comp-sys/controlled`，测试路径无需调整。

#RECORD 4 遇到了问题:独立 workspace 已能编译到 `analysis`，但新版 nightly 的 `rustc_private`/MIR API 与旧实现不兼容，当前有 13 个编译错误，涉及 `Constant`、MIR variant、属性查询、impl 查询和诊断 API。做了改动:暂未迁移 API；已确认这才是 #3636 的核心修复工作。其他:维护策略应为固定受支持 nightly、按该版本编译器源码迁移、为 MIR 语义补回归测试，并在 CI 中持续验证；升级 nightly 时必须检查该工具兼容性。

#RECORD 5 遇到了问题:新版 rustc 将 `Constant` 改为 `ConstOperand`、`InstanceDef` 改为 `InstanceKind`，并重构了 MIR、属性、impl 与诊断接口。做了改动:依据 `nightly-2026-07-21`（commit `87e5904f5`）的源码迁移 `analysis/src/lib.rs` 第一版实现。其他:需要在容器重新运行 `cargo test` 验证并处理剩余编译错误，再确认访问检查语义的 fixture 测试。

#RECORD 6 遇到了问题:`analysis` 已成功编译，但 `component-driver` 仍有 14 个新版 `rustc_private` driver API 错误，涉及 `RunCompiler`、`Queries`、`psess_created`、depinfo、ICE 诊断与 `ExitCode`。做了改动:暂无 driver 代码改动。其他:下一步应以当前编译器源码中的 driver 用法为准，优先恢复“运行 cargo check 后执行访问分析”的核心路径，再处理自定义 ICE 输出与警告清理。

#RECORD 7 遇到了问题:单 crate fixture 被 Cargo 错误归属到仓库根 workspace，导致测试准备阶段的 `cargo clean` 失败。做了改动:为 `duplicate_lib_name_test` 和 `missing_toml_test` 添加独立空 `[workspace]`；完成 driver API 迁移。其他:容器中 `cargo test` 已全部通过；仍有 `once_cell` feature 和生命周期写法的非阻塞警告可后续清理。
