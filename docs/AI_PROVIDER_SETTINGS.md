# AI 服务商设置：改造说明（v0.8.0）

> 这份文档写给后续接手的人/模型。它描述**这次改了什么、为什么这么改、还有哪些没用真机验过**。
> 界面目标是复刻 Hatoba 的服务商对话框；数据目标是摆脱「只能填几家官方预设」。

---

## 1. 为什么改

v0.7 的服务商表单有三处硬伤：

1. **协议只有两个硬编码值**（`open_ai` / `anthropic`），第三个协议装不进来。
2. **每个服务商只能有一个 `model` 字符串**，而同一个端点通常有十几个模型，用户要么背 ID 要么改代码。
3. **基础 URL 不能带版本**。后端偷偷拼 `/v1`，用户填 `https://openrouter.ai/api/v1` 就会拼成 `/api/v1/v1`。
4. 没有「拉取模型列表」和「测试连接」，配错了只能等到聊天时才报错。

同时，本地推理服务器（Ollama、LM Studio、vLLM）**不需要 API key**，而旧代码无条件发 `Authorization: Bearer `（空值），部分反代会直接 400。

---

## 2. 数据模型

存储仍然是保险库里的一个 `kind = "provider"` 条目，payload 明文结构如下（落库前整体加密）：

```jsonc
{
  "id": "uuid",
  "name": "OpenAI",
  "protocol": "chat_completions",   // chat_completions | responses | anthropic
  "base_url": "https://api.openai.com/v1",  // 含 API 版本
  "models": ["gpt-4o", "gpt-4o-mini"],       // 有序、去重
  "api_key": "sk-..."                        // 可为空
}
```

### 与 v0.7 的兼容（重要）

v0.7 存的是 `"model": "gpt-4o"`（单数、字符串）和**不带版本**的 `base_url`。
**没有做数据迁移**，而是在读取时兼容：

| 旧值 | 读取时的处理 |
|---|---|
| `"model": "x"`（无 `models`） | 折成 `models: ["x"]` |
| `protocol: "open_ai"` | `Protocol::parse` 映射为 `ChatCompletions` |
| `base_url: "https://api.openai.com"` | `normalize_base_url` 补 `/v1` |

对应函数：`provider_from_payload` / `load_provider`（commands.rs）、`Protocol::parse` / `normalize_base_url`（ai/provider.rs）。
**改动时不要删掉这些兼容分支**，否则老用户的服务商会失效。

---

## 3. IPC 契约

### 3.1 类型

```ts
// src/lib/api.ts
type ProviderProtocol = "chat_completions" | "responses" | "anthropic";

interface ProviderView {   // 后端 → 前端
  id: string;
  name: string;
  protocol: ProviderProtocol;
  baseUrl: string;
  models: string[];
  hasKey: boolean;         // 只有布尔，密钥永不下发
}

interface ProviderInput {  // 前端 → 后端
  id?: string;
  name: string;
  protocol: string;
  baseUrl: string;
  models: string[];
  apiKey?: string;         // 留空 = 沿用已存的密钥
}
```

### 3.2 命令

| 命令 | 入参 | 出参 | 说明 |
|---|---|---|---|
| `ai_providers_list` | — | `ProviderView[]` | 列出 |
| `ai_provider_save` | `{ input: ProviderInput }` | `ProviderView` | 保存前用 `validate_base_url` 校验 |
| `ai_provider_delete` | `{ id }` | `void` | |
| **`ai_provider_models`** | `{ input: ProviderInput }` | `string[]` | **新增**。`GET {base}/models`，不落库 |
| **`ai_provider_test`** | `{ input: ProviderInput }` | `string` | **新增**。有模型就发一次最小对话，没有就只测列表 |
| `ai_send` | `{ providerId, model?, sessionId, mode, history }` | `void` | **新增 `model` 参数** |

`ai_provider_models` / `ai_provider_test` 都接受**未保存的草稿**，`config_from_input` 负责在 `apiKey` 为空时按 `id` 回读已存密钥——所以「先测通再保存」是可行的。

### 3.3 协议与端点

| protocol | 请求端点 | 应用场景 |
|---|---|---|
| `chat_completions` | `POST {base}/chat/completions` | OpenAI、OpenRouter、DeepSeek、Moonshot、Ollama、LM Studio、vLLM、任何兼容端点 |
| `responses` | `POST {base}/responses` | OpenAI Responses API |
| `anthropic` | `POST {base}/messages` | Anthropic Messages |

`{base}` 由 `normalize_base_url` 保证带版本段（`/v1`、`/v1beta`、`/api/v1` 都认）。

---

## 4. 前端结构

`src/features/settings/ProvidersSection.tsx`（重写）

- **列表态**：每个服务商一行，显示 `协议 · baseUrl` + `N 个模型：…` + `未设置密钥` 提示；右边「编辑 / 删除」。
- **对话框态**：点「添加服务商」或「编辑」打开嵌套 `Modal`（复用 `components/ui.tsx` 的 `Modal`），字段按 Hatoba 的**左标签 + 右控件**两列布局：
  - 名称
  - 协议（下拉：OpenAI Chat Completions / OpenAI Responses / Anthropic Messages）
  - 基础 URL + 提示「地址要包含 API 版本」+ **快速填入**（6 个预设，只改协议和 URL，不改其他）
  - API 密钥 + 明文/密文切换（`IconEye` / `IconEyeOff`）
  - 模型区：「获取模型列表」按钮 → 结果作为可点击 chip；已选模型列表（带删除）；手动输入框 + 「添加」
  - 底部：「测试连接」+「取消 / 保存」
- 结果提示分两种颜色：成功走 `.notice--ok`，失败走 `.error`。

`src/features/ai/AiPanel.tsx`

- 服务商下拉换成带 `models` 的类型；**新增模型下拉**，仅在该服务商有 >1 个模型时出现。
- 切换服务商时**重置模型**为该服务商的第一个（不同端点的模型 ID 不通用）。
- `aiApi.send` 多传一个 `model` 参数。

样式在 `src/styles/app.css` 末尾新增一节 `.pfield` / `.model-list` / `.model-item` / `.fetched` / `.preset-row` / `.chip--sm` / `.notice--ok` 等。

---

## 5. 后端要点（`src-tauri/src/ai/provider.rs`）

| 函数 | 作用 |
|---|---|
| `Protocol::parse(&str)` | 兼容 `open_ai` 旧值；未知值回退 `chat_completions` |
| `normalize_base_url` | 补版本段；`/v1`、`/v1beta`、`/api/v1` 视为已带版本 |
| `endpoint(base, path)` | 拼端点，避免 `//` |
| `with_auth(req, protocol, key)` | **key 为空时不发认证头**；Anthropic 用 `x-api-key` + `anthropic-version` |
| `list_models(config)` | `GET {base}/models`，兼容 `{data:[…]}` 和裸数组，去重排序 |
| `test_connection(config)` | 有 model → 发一条 "ping"；无 model → 只调 `list_models` |
| `responses_body` / `parse_responses_reply` | Responses 协议的请求体与响应解析，**拆成纯函数以便单测** |

`validate_base_url` 拒绝：空串、非 http(s)、无主机名、URL 里带用户名/密码（避免密钥进日志）。

---

## 6. 验证状态（务必按实际写）

| 项 | 状态 |
|---|---|
| `cargo test` | 74 个通过（provider 模块 10 个） |
| `cargo clippy -D warnings` | 0 警告 |
| `npx tsc --noEmit` | 通过 |
| `chat_completions` 真实端点 | ✅ 对本地 mock（3 个模型）验证：拉列表、选模型、测试连接返回 `pong（模型 mock-gpt-4o-mini 可用）`、保存后列表正确 |
| 保险库仍是密文 | ✅ 对 `loki.db` 全量 dump 检索 `mock-gpt-4o-mini` / `127.0.0.1:18090` / `Mock 本地` 均 0 命中 |
| `anthropic` 协议 | 代码路径与 v0.7 相同（仅 URL/认证助手替换），本轮**未重新真机验证** |
| `responses` 协议 | 请求体/响应解析有单元测试；**没有可用端点，未做真机验证** |
| 老数据迁移 | 兼容分支有逻辑处理与单测（`protocol_parsing_accepts_the_old_spelling` 等），**未用真实旧库回放验证** |

---

## 7. 已知限制 / 后续可做

1. **模型元数据缺失**：没有上下文窗口、最大输出、reasoning 档位等信息。Hatoba 是查 `models.dev` 补齐。要做得更完整，可以加一份内置或远端的能力表。
2. **模型列表不分页**：只发一次 `GET /models`。OpenAI 自家的列表很长但能一次返回；若遇到分页端点（返回 `has_more`/`after`），需要在 `list_models` 里补游标循环（蓝图参考 `models.rs:230-345`，最多 20 页）。
3. **`responses` 协议未实测**：语法按官方文档写，若接真端点报 400，优先检查 `input` 里 `function_call` / `function_call_output` 的形状。
4. **没有「设为默认服务商」**：AI 面板取列表第一个。
5. **没有流式输出**：`complete()` 是非流式，长回答会等整段返回。Hatoba 是流式的，若要做需要改 `run_turn` 与事件通道。
6. **密钥只支持单一认证方式**：Anthropic 固定 `x-api-key`，OpenAI 兼容固定 `Bearer`。若有自定义认证头的服务商（如某些网关），需要加 `auth_header` 字段。

---

## 8. 改动文件清单

```
src-tauri/src/ai/provider.rs              协议枚举 / 配置 / URL 归一 / 认证 / 模型列表 / 测试连接 / Responses
src-tauri/src/commands.rs                 ProviderView·Input 加 models；新增 ai_provider_models、ai_provider_test；ai_send 支持 model；config_from_input
src-tauri/src/lib.rs                      注册两个新命令
src/lib/api.ts                            ProviderProtocol/ProviderView/ProviderInput；aiApi.models/test；aiApi.send 加 model
src/features/settings/ProvidersSection.tsx  重写为 Hatoba 风格对话框
src/features/ai/AiPanel.tsx               模型下拉、切换服务商重置模型
src/styles/app.css                        .pfield / .model-list / .fetched / .preset-row / .chip--sm / .notice--ok
```

---

## 9. 你可以让 Claude 接着做什么（按优先级）

1. 给 `responses` 与 `anthropic` 各补一次真机验证（有可用 key 时）。
2. `list_models` 加分页循环，兼容带 `has_more` 的端点。
3. 加模型能力表（上下文/输出上限），供后续上下文裁剪使用。
4. 加「默认服务商」与「默认模型」并持久化。
5. 用真实旧库（v0.7 结构）回放一次迁移，确认兼容分支没有遗漏。
