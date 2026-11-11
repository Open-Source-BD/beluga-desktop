import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

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
  img({ alt }) {
    return <span>{alt}</span>;
  },
};

export default function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
