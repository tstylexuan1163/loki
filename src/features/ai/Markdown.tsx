import { isValidElement, memo, useState, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { IconCheck, IconCopy } from "@/components/icons";

/**
 * The assistant's reply, rendered as Markdown (CommonMark + GFM tables, task
 * lists, strikethrough).
 *
 * Raw HTML in the model's output is never rendered — react-markdown drops it
 * unless a raw-HTML plugin is added, and none is. Links are sanitised by the
 * library and open in the system browser, never inside the app's WebView.
 */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={COMPONENTS}>
        {text}
      </ReactMarkdown>
    </div>
  );
});

const COMPONENTS: Components = {
  a: ({ href, children }) => (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        if (href && /^https?:\/\//i.test(href)) void openUrl(href).catch(() => {});
      }}
      title={href}
    >
      {children}
    </a>
  ),
  pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
  table: ({ children }) => (
    <div className="md__table">
      <table>{children}</table>
    </div>
  ),
};

/** Pulls the language and the raw text out of the `<code>` inside a `<pre>`. */
function codeOf(children: ReactNode): { lang: string; text: string } {
  const code = Array.isArray(children) ? children[0] : children;
  if (!isValidElement<{ className?: string; children?: ReactNode }>(code)) {
    return { lang: "", text: String(children ?? "") };
  }
  const lang = /language-([\w+#.-]+)/.exec(code.props.className ?? "")?.[1] ?? "";
  const text = String(code.props.children ?? "").replace(/\n$/, "");
  return { lang, text };
}

function CodeBlock({ children }: { children: ReactNode }) {
  const { lang, text } = codeOf(children);
  const [copied, setCopied] = useState(false);
  return (
    <div className="md__code">
      <div className="md__code-head">
        <span className="md__code-lang">{LANG_LABEL[lang] ?? (lang || "代码")}</span>
        <button
          className="md__code-copy"
          onClick={() => {
            void navigator.clipboard.writeText(text).then(() => {
              setCopied(true);
              window.setTimeout(() => setCopied(false), 1500);
            });
          }}
          title="复制"
        >
          {copied ? <IconCheck size={12} /> : <IconCopy size={12} />}
          {copied ? "已复制" : "复制"}
        </button>
      </div>
      <pre>
        <code>{text}</code>
      </pre>
    </div>
  );
}

const LANG_LABEL: Record<string, string> = {
  sh: "Shell",
  bash: "Bash",
  zsh: "Zsh",
  shell: "Shell",
  console: "终端",
  ps1: "PowerShell",
  powershell: "PowerShell",
  js: "JavaScript",
  ts: "TypeScript",
  py: "Python",
  python: "Python",
  yml: "YAML",
  yaml: "YAML",
  json: "JSON",
  ini: "INI",
  conf: "配置",
  nginx: "Nginx",
  sql: "SQL",
  rust: "Rust",
  go: "Go",
  dockerfile: "Dockerfile",
  diff: "Diff",
};
