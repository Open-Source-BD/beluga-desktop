import type { ReactNode } from "react";

/** The empty state: the greeting above the message box, which arrives as children. */
export default function EmptyChat({ children }: { children: ReactNode }) {
  return (
    <div className="empty">
      <div className="col">
        <h2 className="hello">What are we building?</h2>
        <p className="hello-sub">
          Ask a question, paste an error, or think a problem through.
        </p>
      </div>
      {children}
    </div>
  );
}
