import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import App from "./App";
import { APP_NAME } from "./brand";

describe("App", () => {
  it("renders the app name as a heading", () => {
    render(<App />);
    expect(screen.getByRole("heading", { name: APP_NAME })).toBeInTheDocument();
  });
});
