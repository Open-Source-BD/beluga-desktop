import { isValidElement, memo, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import CodeBlock from "./CodeBlock";

function textOf(node: ReactNode): string {
  if (typeof node === "string") return node;
  if (Array.isArray(node)) return node.map(textOf).join("");
  return "";
}

// No raw HTML (no rehype-raw) and no navigation: a link shows its text and its
// address as plain text, and an image shows only its description.
const components: Components = {
  a({ href, children }) {
    const label = typeof children === "string" ? children : undefined;
    if (!href || label === href) return <span>{children}</span>;
    return (
      <span>
        {children} ({href})
      </span>
    );
  },
  // Only fenced code arrives here as <pre><code>; inline code stays a plain <code>.
  pre({ children }) {
    if (!isValidElement<{ className?: string; children?: ReactNode }>(children)) {
      return <pre>{children}</pre>;
    }
    const language = /language-(\S+)/.exec(children.props.className ?? "")?.[1] ?? "";
    return <CodeBlock language={language} code={textOf(children.props.children).replace(/\n$/, "")} />;
  },
  img({ alt }) {
    return <span>{alt}</span>;
  },
};

function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}

// Settled replies do not render again while another part of the thread streams.
export default memo(Markdown);
