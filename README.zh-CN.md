<p align="center">
  <img src="web/public/meido-crest.svg" alt="Meido 徽标" width="180">
</p>

<h1 align="center">Meido</h1>

<p align="center"><strong>不止于单线程聊天机器人：面向个人智能体的操作系统。</strong></p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.ja.md">日本語</a>
</p>

Meido 让个人智能体拥有连续的工作上下文：知道当前重点、记得过往经历、理解知识库，
并在执行下一步行动时遵循既定操作规程。它将多维上下文管理、持久化执行能力和由用户
掌控的运行环境整合在一起，而不是把每次对话都当作孤立的问答。

一个 Rust 运行时统一编排 **40+ 个模块化技能**、**35+ 个 MCP 工具**、定时任务以及
Web 和聊天渠道中的多种智能体引擎。所有渠道都进入同一条智能体执行路径，因此记忆、
权限、安全策略和可追溯性始终保持一致。

## 为什么选择 Meido

大多数助手的记忆止步于聊天窗口。Meido 提供一套完整的工作上下文系统：

- **三层会话记忆** — 将长期身份与原则、进行中的项目、近期关注点分层保存，避免短期
  噪声覆盖长期信息。
- **MemPalace 召回** — 通过语义检索找回相关知识，无需把整个资料库塞进每次提示词。
- **Bulletin 当前焦点** — 每个 Persona 都有一条简明、可见的当前重点。
- **Obsidian 知识库** — 知识以可读、可移植、可直接编辑的 Markdown 保存；Meido
  可以搜索、索引并操作这些内容。
- **可追溯的智能体历史** — 追加式记忆事件和运行产物让智能体行为可以被检查，而不是
  黑箱。

## 一个运行时，多种交互界面

- **统一智能体路径** — Web、Telegram、Discord、WhatsApp、企业微信和定时任务使用
  同一个编排入口。
- **多智能体引擎** — 每个 Persona 可选择 Classic、Cursor 或 Gemini ADK，同时保留
  一致的上下文和控制机制。
- **模块化能力层** — 按需激活技能、连接 MCP 服务，无需为每种工作流单独开发机器人。
- **定时与后台工作** — 周期任务和长任务复用实时对话中的 Persona、记忆、工具与投递路径。
- **Web 操作台** — 在一个界面中管理 Persona、模型、渠道、Hook、技能、日程、记忆和
  系统状态。

## 安全是系统的一部分

Meido 不把安全当作提示词末尾的一句提醒。知识库 SOP 会注入智能体上下文；工具权限在
服务端校验；密钥始终保持遮盖并经过脱敏；文件路径按 Persona 限定；重要操作可通过
智能体历史和产物进行复查。

## 核心能力

- 本地工作区访问：文件、Shell、浏览器和代码仓库感知编辑
- Obsidian 搜索、语义召回、自动索引和 Markdown 原生知识管理
- 按 Persona 配置记忆、模型、Hook、技能、权限和外部路径
- 多 Persona 协作、访客回合、共享看板和后台交接
- 支持 Anthropic、OpenAI、Gemini、本地模型及其他已配置连接器
- 通过 Web、Telegram、Discord、WhatsApp 和企业微信投递
- Cron 风格定时、后台任务、状态跟踪和结果投递
- 可扩展技能和 MCP 工具，并由 SOP 提供操作安全边界

## 快速开始

### 1. 安装

Linux / macOS：

```bash
curl -fsSL https://raw.githubusercontent.com/kenpyfin/meido/main/install.sh | bash
```

Windows 请使用 [`install.ps1`](install.ps1)。从源码构建：

```bash
cargo build --release
cd web && npm ci && npm run build
```

### 2. 初始化配置

Meido 从当前目录的 `.env` 读取配置；也可以通过 `MEIDO_CONFIG` 指定路径。

```bash
cp .env.example .env
```

只需先设置工作区和 Web 启动参数。LLM、知识库、MemPalace、Steel Browser、渠道和
Persona 可在 Web 设置向导中完成配置。请勿提交 `.env`。

### 3. 启动

```bash
meido start
```

打开 **http://127.0.0.1:10961**。`meido start` 会在后台运行；使用 `meido stop`
停止，或添加 `--foreground` 保持前台运行。

Web UI 本身即可独立使用，Telegram、Discord、WhatsApp 和企业微信均为可选渠道。
至少配置一个云端模型 API 密钥，或选择无需密钥的本地模型服务。

### 4. 可选的登录自启动服务

macOS 和 Linux 可安装用户级服务：

```bash
meido gateway install
meido gateway status
meido gateway logs
```

## 记忆模型

每个聊天和 Persona 的规范记忆保存在：

- `groups/{chat_id}/{persona_id}/memory_state.json` — 当前三层记忆状态
- `groups/{chat_id}/{persona_id}/memory_events.jsonl` — 仅追加的审计与恢复事件

`memory_state.json` 包含身份、长期记忆、活跃项目、近期关注、工作模式记忆以及
MemPalace 引用。写入前会进行规范化与验证，并使用临时文件、原子重命名和备份保护数据。

完整配置项请参阅 [`.env.example`](.env.example)，更详细的字段定义和运维说明请查看
[英文 README](README.md)。

## 许可证

混合许可证，详见 [LICENSE](LICENSE)。

- **kenpyfin 在 2026 年的修改与新增文件：** 专有、源码可见；允许个人非商业使用，
  不允许复制、再分发或重新授权这些修改。
- **源自上游的部分：** MIT 许可证，并按要求保留署名。
