<div align="center">

<img src="assets/logo.png" alt="MacCV" width="128" height="128">

# MacCV

**快、轻、私密的 macOS 剪贴板。**

菜单栏剪贴板管理器，Rust 引擎 + SwiftUI 界面。它读写的是**原生剪贴板**（不只是纯文本），
并且可以在**完全不经过剪贴板**的情况下把内容送进应用。

[![CI](https://github.com/yxxbc/Maccv/actions/workflows/ci.yml/badge.svg)](https://github.com/yxxbc/Maccv/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/yxxbc/Maccv?include_prereleases&sort=semver)](https://yxxbc.github.io/Maccv/changelog/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![macOS](https://img.shields.io/badge/macOS-27%2B-black.svg)](#环境要求)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

[English](README.md) · [简体中文](README.zh-CN.md) · [官方网站](https://yxxbc.github.io/Maccv/)

</div>

---

## 三个核心理念

MacCV 建立在三个承诺之上。它们具体到可以被验证；凡是 macOS 本身有限制的地方，
本文会直说，而不是含糊其辞。

### 一、快

速度不是后来加上的特性，它直接决定了架构。引擎用 Rust 编写，编译成**静态库直接链入应用**，
因此没有 IPC 往返、没有序列化、没有辅助进程。变更检测耗时 **0.53 µs** —— 这正是轮询方案
可行的原因；而轮询让 MacCV 无需事件监听，也就不必为了「感知一次复制」而索取辅助功能权限。

实测数据（由测试断言，而非手工测一次）：

| 操作 | 耗时 |
|---|---|
| 检测变更（`changeCount`） | **0.53 µs** |
| 读取纯文本 | **9.9 µs** |
| 感知外部 `pbcopy` 写入 | **116 µs** |
| 写入文本 | 215 µs —— 这是 AppKit 自身的开销，系统要通知每个关心的进程 |

### 二、轻

闲置的剪贴板管理器应该是不存在的。MacCV 的监听线程在复制发生后以 400 µs 间隔轮询（人不可感知），
在无变化时按几何级数退避到 250 ms。UI 层没有任何定时器。历史记录保存在内存中，
Release 应用体积 **1.9 MB**。

### 三、私密

这里必须说清版本，也是多数剪贴板管理器夸大其词的地方。

**一个不舒服的事实：** 内容一旦进入系统剪贴板，**任何** macOS 应用都能读取。
这不是缺陷，也不是缺失的功能，而是 macOS 的设计。**没有任何 API 可以把内容写进剪贴板而不被他人读取。**
`NSPasteboardContentsOptionsCurrentHostOnly` 阻止的是同步到你的其他设备，不是其他应用。
AppKit 里根本不存在 `concealed` / `transient` 标志 —— `org.nspasteboard.ConcealedType`
只是第三方约定，靠应用自愿遵守；而 `NSPasteboardAccessBehavior` 管的是**你自己**能不能读，
从来管不着别人。

所以 MacCV 做那件真正有效的事：**它可以完全不使用剪贴板来粘贴。**

开启「不经过剪贴板粘贴」后，从记录的右键菜单选择它，内容会通过
`CGEventKeyboardSetUnicodeString` 以合成按键直接输入到最前面的应用。
内容全程不进入任何共享通道，因此其他进程**在物理上无法**观察到它。这是真实的保证，不是一句承诺。

代价也是真实的，所以它默认关闭、由你选择：

- 需要**辅助功能权限** —— 因为向其他应用注入事件正是这个权限所管辖的能力。
- **更慢** —— 事件按打字速度投递。
- **仅限文本** —— 图片没有对应的按键表示，仍走剪贴板路径。

在此之外，那些完全在应用掌控范围内的措施：

| 措施 | 真正保证的是什么 |
|---|---|
| 历史记录只存在应用容器内 | 其他应用读不到你的历史。共享的永远只有那一次剪贴板写入，而不是整份记录。 |
| 跳过隐藏条目 | 密码管理器标记为 `ConcealedType` 的条目**永不记录** —— 这是尊重明确意图，不是猜测。 |
| 敏感内容打码 | 检测到的卡号、API 密钥、JWT、私钥、带凭据的连接串、邮箱、身份证号，在**记录时就**打码，而不是只在下显示时打码。 |
| 替换自身前必须确认 | 下载的更新先校验、再呈现。MacCV 绝不静默改写自己的包体。 |

**MacCV 不做声明的是：** 它无法阻止其他应用读取剪贴板 —— 没有东西能做到。
如果你需要这个，请使用「不经过剪贴板粘贴」，或者不要把机密放上剪贴板。
本文把这点写明，因为**一个你无法验证的隐私声明，比没有声明更糟**。

## 支持的内容类型

macOS 定义的每一种剪贴板类型，而非只有纯文本：

| 类别 | 类型 |
|---|---|
| 文本 | `public.utf8-plain-text`、`public.utf8-tab-separated-values-text` |
| 富文本 | `public.rtf`、`com.apple.flat-rtfd`、`public.html` |
| 图片 | `public.png`、`public.tiff`、`com.adobe.pdf` |
| 文件 | `public.file-url` —— 多文件复制保持多条目 |
| 链接 | `public.url` |
| 颜色 | `com.apple.cocoa.pasteboard.color`，解析为 sRGB |
| 声音 | `com.apple.cocoa.pasteboard.sound` |
| 字体与标尺 | `com.apple.cocoa.pasteboard.font`、`…ruler` |
| 其他 | 自定义 UTI，原样保存、原样回放 |

一次复制可以同时携带多种格式 —— 从 Safari 复制一段文字会同时带来纯文本、RTF **和** HTML ——
MacCV 会全部保留，因此无论目标应用要哪种格式都能正确粘贴。

## 设置

- **通用** —— 记录条数上限，界面语言（跟随系统，或固定为 English / 简体中文）。
- **隐私** —— 敏感内容打码、保留可辨认提示、跳过隐藏条目、不经过剪贴板粘贴，以及辅助功能权限状态。
- **更新** —— 自动 / 仅提示 / 手动、可选的预发布通道，以及「检查更新」。
- **关于** —— 版本、构建号、引擎版本与相关链接。

## 更新机制

「检查更新」读取发布在[官网](https://yxxbc.github.io/Maccv/latest.json)上的清单。
安装过程严格按固定顺序执行，**顺序本身就是重点**：

1. 下载到暂存目录
2. 校验 **SHA-256** —— 不匹配则删除文件并终止
3. 用 `spctl` 校验**代码签名**
4. **只有前两步都通过后**才清除 `com.apple.quarantine`
5. 如实报告每一步的结论

第 4 步正是避免出现臭名昭著的「**MacCV 已损坏，无法打开**」对话框的关键。
一个下载自身新版本的应用会从父进程继承隔离属性，而 macOS 拒绝启动被隔离且未公证的包。
如果在校验**之前**清除该属性，就等于彻底废掉了它的意义 —— 所以签名校验不通过的更新会被
如实报告并保持隔离状态，而不是悄悄装上。

MacCV **不**替换自己正在运行的包体。在一个应用运行时做半途而废的替换，比请你手动拖一个文件更糟；
因此校验通过、已解除隔离的包体会在访达中显示出来，由你完成最后一步。

## 环境要求

- macOS 27 或更高版本
- 构建需要：Xcode 27、Rust 1.85+、[XcodeGen](https://github.com/yonaskolb/XcodeGen)

## 构建与运行

```bash
git clone https://github.com/yxxbc/Maccv.git
cd Maccv

# Rust 引擎：可独立构建与测试
cargo test
cargo build --release -p maccv-ffi

# 应用（先生成 MacCV.xcodeproj，再构建）
cd apps/mac
xcodegen generate
xcodebuild -scheme MacCV -configuration Release build
```

`apps/mac/project.yml` 是 Xcode 工程的唯一事实来源；`.xcodeproj` 是生成产物，不纳入版本控制。
构建阶段会自动编译并链接 Rust 引擎，所以你只需要执行 Xcode 构建。

### 验证构建

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test                                  # 109 个测试
xcodebuild -scheme MacCV build

# 编译出的应用会端到端自检整条链路：
"$BUILD_DIR/MacCV.app/Contents/MacOS/MacCV" --self-test
```

自检使用**进程私有**剪贴板，因此绝不会读取或覆盖你真实的剪贴板，
CI 也不依赖运行器上恰好复制了什么。

`Scripts/publish.sh` 会创建 GitHub 仓库、设置描述与标签并推送。
它被刻意做成需要你主动运行的脚本，因为「对外发布」是一个动作，而不是构建步骤。

## 项目结构

```
crates/maccv-core/       Rust 引擎 —— 核心部分
  pasteboard.rs            NSPasteboard 读写、快照、监听
  content.rs               类型化视图与内容分类
  secure_input.rs          合成按键（不经过剪贴板粘贴）
  redaction.rs             敏感内容识别与打码
  updater.rs               更新检查、校验、解除隔离
crates/maccv-ffi/        C ABI 桥接层；生成 include/maccv.h
apps/mac/                SwiftUI 应用；project.yml 是工程的事实来源
  Resources/Localizable.xcstrings   英文与简体中文
docs/                    官网（GitHub Pages）
Scripts/                 build-rust.sh、changelog-notes.sh、publish.sh
assets/                  Logo 与社交预览图（SVG + 渲染产物）
```

### 两半是如何连接的

`maccv-core` 是纯 Rust，不掺任何 C 相关的东西，因此可以直接使用与测试。
`maccv-ffi` 是一个轻薄的 `staticlib`，把它以 C ABI 暴露出去；`cbindgen` 在构建时生成 `maccv.h`。
Swift 通过桥接头文件导入该头文件，直接调用进 Rust —— 同一地址空间，没有编组层。

三个值得了解的决定：

**监听线程自己打开句柄。** `objc2` 刻意不为 `NSObject` 子类实现 `Send`。
与其用 `unsafe impl` 强行绕过，监听线程按名字重新打开剪贴板，只有变更计数跨越线程边界。

**快照是一次分配，并且自己持有字节。** C ABI 返回单个内存块，内含条目数组、表示数组、
载荷字节与字符串。释放快照即释放全部内容，且不存在指向已释放 Rust 内存的悬垂指针。

**打码发生在写入时。** 在记录条目时打码（而非绘制时），意味着存储的预览与载荷本身也已被打码 ——
所以无论是截屏面板，还是查看进程内存，都不会暴露原值。

## macOS 27 与剪贴板隐私

macOS 27 对通用剪贴板的程序化访问实施了管控。应用第一次读取时系统可能弹出询问，
而用户的回答可以被永久记住为「永不允许」。

MacCV 把这当作一种正常状态，而不是反复重试的错误：引擎返回 `AccessDenied`，
面板解释发生了什么并提供跳转到对应系统设置面板的入口，绝不做任何绕过拒绝的循环轮询。

## 路线图

- [x] 全类型剪贴板读写、无损往返
- [x] 自适应轮询的变更监听
- [x] 多条目保真（*n* 个文件保持 *n* 个条目）
- [x] SwiftUI 菜单栏面板
- [x] 不经过剪贴板粘贴
- [x] 敏感内容打码
- [x] 应用内更新（签名校验 + 解除隔离属性）
- [x] 英文与简体中文界面
- [ ] 磁盘持久化历史（SQLite），重启后保留
- [ ] 全局快捷键
- [ ] 面板的拖放导入与导出
- [ ] 固定条目、按应用规则
- [ ] 代码公证，使其打开时不再出现 Gatekeeper 提示

## 参与贡献

欢迎提交 issue 与 pull request —— 请见 [CONTRIBUTING.md](CONTRIBUTING.md) 与[行为准则](CODE_OF_CONDUCT.md)。

`crates/maccv-core/tests/clipboard.rs` 中的测试针对真实的 `NSPasteboard` 运行
（进程私有的那个，不会动你的剪贴板），它们是对预期行为最好的描述。

## 许可证

[Apache-2.0](LICENSE) © yxxbc

## 致谢

构建于 [`objc2`](https://github.com/madsmtm/objc2) 之上 —— 正是它让用 Rust 写 AppKit 引擎
变得舒适而非危险。
