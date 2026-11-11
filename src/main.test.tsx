import { expect, it, vi } from "vitest";
import { APP_NAME } from "./app/brand";

vi.mock("react-dom/client", () => ({
  default: { createRoot: () => ({ render: () => {} }) },
}));
vi.mock("./app/App", () => ({ default: () => null }));

it("sets the document title from APP_NAME at startup", async () => {
  document.title = "";
  document.body.innerHTML = '<div id="root"></div>';
  await import("./main");
  expect(document.title).toBe(APP_NAME);
});
