import { describe, expect, it } from "vitest";
import confText from "../../src-tauri/tauri.conf.json?raw";
import indexHtml from "../../index.html?raw";
import { APP_NAME } from "./brand";

const conf = JSON.parse(confText);
const main = conf.app.windows[0];

describe("main window", () => {
  it("opens at 1180x760 and cannot shrink below 720x520", () => {
    expect(main).toMatchObject({ width: 1180, height: 760, minWidth: 720, minHeight: 520 });
  });

  it("names the app only through productName, not the window or page title", () => {
    expect(conf.productName).toBe(APP_NAME);
    expect(main.title).toBeUndefined();
    expect(indexHtml).not.toContain(APP_NAME);
  });
});
