import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import Composer, { type ComposerControl } from "./Composer";

const box = () => screen.getByLabelText("Message") as HTMLTextAreaElement;
const paste = (el: Element, text: string, files: File[] = []) =>
  fireEvent.paste(el, { clipboardData: { getData: () => text, files, types: ["text/plain"] } });

afterEach(() => {
  vi.useRealTimers();
  localStorage.clear();
});

describe("Composer send key", () => {
  it("with the modifier setting on, Enter adds a new line and Cmd/Ctrl+Enter sends", async () => {
    const onSend = vi.fn();
    render(<Composer onSend={onSend} sendWithModifier />);
    await userEvent.type(box(), "one{Enter}two");
    expect(onSend).not.toHaveBeenCalled();
    expect(box()).toHaveValue("one\ntwo");
    await userEvent.keyboard("{Control>}{Enter}{/Control}");
    expect(onSend).toHaveBeenCalledWith("one\ntwo", []);
  });

  it("Cmd+Enter sends too", async () => {
    const onSend = vi.fn();
    render(<Composer onSend={onSend} sendWithModifier />);
    await userEvent.type(box(), "go{Meta>}{Enter}{/Meta}");
    expect(onSend).toHaveBeenCalledWith("go", []);
  });

  it("by default Enter sends and Ctrl+Enter also sends", async () => {
    const onSend = vi.fn();
    render(<Composer onSend={onSend} />);
    await userEvent.type(box(), "a{Enter}");
    expect(onSend).toHaveBeenCalledWith("a", []);
  });

  it("Enter still picks from an open menu in modifier mode", async () => {
    const onSend = vi.fn();
    const onKeyIntercept = vi.fn((e: { key: string }) => e.key === "Enter");
    render(<Composer onSend={onSend} sendWithModifier onKeyIntercept={onKeyIntercept} />);
    await userEvent.type(box(), "/he{Enter}");
    expect(onSend).not.toHaveBeenCalled();
    expect(box()).toHaveValue("/he");
  });
});

describe("Composer Up arrow", () => {
  it("in an empty box calls onEmptyUp", async () => {
    const onEmptyUp = vi.fn();
    render(<Composer onSend={vi.fn()} onEmptyUp={onEmptyUp} />);
    await userEvent.type(box(), "{ArrowUp}");
    expect(onEmptyUp).toHaveBeenCalledTimes(1);
  });

  it("with text in the box it only moves the caret", async () => {
    const onEmptyUp = vi.fn();
    render(<Composer onSend={vi.fn()} onEmptyUp={onEmptyUp} />);
    await userEvent.type(box(), "abc{ArrowUp}");
    expect(onEmptyUp).not.toHaveBeenCalled();
  });

  it("does nothing when a menu took the key", async () => {
    const onEmptyUp = vi.fn();
    render(<Composer onSend={vi.fn()} onEmptyUp={onEmptyUp} onKeyIntercept={() => true} />);
    await userEvent.type(box(), "{ArrowUp}");
    expect(onEmptyUp).not.toHaveBeenCalled();
  });
});

describe("Composer paste", () => {
  it("3,999 characters go into the box", async () => {
    render(<Composer onSend={vi.fn()} />);
    box().focus();
    await userEvent.paste("x".repeat(3999));
    expect(box()).toHaveValue("x".repeat(3999));
    expect(screen.queryByText("Pasted text.txt")).toBeNull();
  });

  it("4,000 characters become a Pasted text.txt chip and leave the box unchanged", async () => {
    render(<Composer onSend={vi.fn()} />);
    await userEvent.type(box(), "before");
    paste(box(), "y".repeat(4000));
    expect(await screen.findByText("Pasted text.txt")).toBeInTheDocument();
    expect(box()).toHaveValue("before");
  });

  it("a second one is Pasted text 2.txt", async () => {
    render(<Composer onSend={vi.fn()} />);
    paste(box(), "y".repeat(4000));
    paste(box(), "z".repeat(4000));
    expect(await screen.findByText("Pasted text 2.txt")).toBeInTheDocument();
  });

  it("Put in the message removes the chip and inserts the text at the caret", async () => {
    render(<Composer onSend={vi.fn()} />);
    await userEvent.type(box(), "ab");
    box().setSelectionRange(1, 1);
    const big = "q".repeat(4000);
    paste(box(), big);
    await userEvent.click(await screen.findByRole("button", { name: "Put in the message" }));
    expect(box()).toHaveValue(`a${big}b`);
    expect(screen.queryByText("Pasted text.txt")).toBeNull();
  });

  it("a pasted text chip is sent as an attachment", async () => {
    const onSend = vi.fn();
    render(<Composer onSend={onSend} />);
    paste(box(), "y".repeat(4000));
    await screen.findByText("Pasted text.txt");
    await userEvent.type(box(), "see{Enter}");
    expect(onSend.mock.calls[0][1][0]).toMatchObject({ name: "Pasted text.txt", mime: "text/plain" });
  });

  it("pasting an image attaches it", async () => {
    render(<Composer onSend={vi.fn()} />);
    paste(box(), "", [new File([new Uint8Array([1, 2])], "image.png", { type: "image/png" })]);
    expect(await screen.findByText("image.png")).toBeInTheDocument();
    expect(box()).toHaveValue("");
  });

  it("a pasted image is refused when the model cannot read images", async () => {
    render(<Composer onSend={vi.fn()} acceptsImages={false} />);
    paste(box(), "", [new File([new Uint8Array([1, 2])], "image.png", { type: "image/png" })]);
    expect(await screen.findByRole("alert")).toHaveTextContent("This model cannot read images.");
  });

  it("a pasted text chip past the attachment limit shows the existing refusal and leaves the box alone", async () => {
    render(<Composer onSend={vi.fn()} />);
    for (let i = 0; i < 6; i++) paste(box(), `${i}`.repeat(4000));
    expect(await screen.findByRole("alert")).toHaveTextContent("You can attach up to 5 files.");
    expect(box()).toHaveValue("");
  });
});

describe("Composer addFiles (drop)", () => {
  it("attaches every dropped file and shows the existing refusal for a bad one", async () => {
    const ref = createRef<ComposerControl>();
    render(<Composer onSend={vi.fn()} controlRef={ref} />);
    await act(async () => {
      await ref.current?.addFiles([
        new File(["a"], "a.txt", { type: "text/plain" }),
        new File(["b"], "b.txt", { type: "text/plain" }),
        new File(["PK"], "c.zip", { type: "application/zip" }),
      ]);
    });
    expect(screen.getByText("a.txt")).toBeInTheDocument();
    expect(screen.getByText("b.txt")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("c.zip is not a text file or image.");
  });

  it("focus() puts the caret in the box", () => {
    const ref = createRef<ComposerControl>();
    render(<Composer onSend={vi.fn()} controlRef={ref} />);
    act(() => ref.current?.focus());
    expect(box()).toHaveFocus();
  });
});

describe("Composer drafts", () => {
  it("saves after 300 ms, restores on return to the chat, and clears after a successful send", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const onSend = vi.fn();
    const { rerender } = render(<Composer onSend={onSend} draftId="c1" />);
    await userEvent.type(box(), "half done");
    expect(localStorage.getItem("draft:c1")).toBeNull();
    act(() => void vi.advanceTimersByTime(300));
    expect(localStorage.getItem("draft:c1")).toBe("half done");

    rerender(<Composer onSend={onSend} draftId="c2" />);
    expect(box()).toHaveValue("");
    rerender(<Composer onSend={onSend} draftId="c1" />);
    expect(box()).toHaveValue("half done");

    await userEvent.type(box(), "{Enter}");
    await waitFor(() => expect(box()).toHaveValue(""));
    act(() => void vi.advanceTimersByTime(300));
    expect(localStorage.getItem("draft:c1")).toBeNull();
  });

  it("a new chat uses draft:new", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    render(<Composer onSend={vi.fn()} draftId={null} />);
    await userEvent.type(box(), "idea");
    act(() => void vi.advanceTimersByTime(300));
    expect(localStorage.getItem("draft:new")).toBe("idea");
  });

  it("switching chats inside the debounce keeps the text under its own chat", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { rerender } = render(<Composer onSend={vi.fn()} draftId="c1" />);
    await userEvent.type(box(), "quick");
    rerender(<Composer onSend={vi.fn()} draftId="c2" />);
    expect(localStorage.getItem("draft:c1")).toBe("quick");
    act(() => void vi.advanceTimersByTime(300));
    expect(localStorage.getItem("draft:c2")).toBeNull();
  });

  it("keeps the draft when the send is rejected", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    localStorage.setItem("draft:c1", "kept");
    const onSend = vi.fn().mockRejectedValue(new Error("no"));
    render(<Composer onSend={onSend} draftId="c1" />);
    await userEvent.type(box(), "{Enter}");
    act(() => void vi.advanceTimersByTime(300));
    expect(localStorage.getItem("draft:c1")).toBe("kept");
  });

  it("ignores storage errors", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    render(<Composer onSend={vi.fn()} draftId="c1" />);
    await userEvent.type(box(), "still works");
    expect(box()).toHaveValue("still works");
    vi.restoreAllMocks();
  });

  it("without a draftId nothing is stored", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    render(<Composer onSend={vi.fn()} />);
    await userEvent.type(box(), "x");
    act(() => void vi.advanceTimersByTime(500));
    expect(localStorage.length).toBe(0);
  });
});

