import { describe, expect, it } from "vitest";
import { enUS, zhCN } from "./messages";

describe("i18n messages", () => {
  it("keeps Chinese and English message keys in sync", () => {
    expect(Object.keys(enUS).sort()).toEqual(Object.keys(zhCN).sort());
  });

  it("contains the active language labels used by the shell switcher", () => {
    expect(zhCN["app.language"]).toBe("语言");
    expect(enUS["app.language"]).toBe("Language");
    expect(zhCN["nav.workbench"]).toBe("工作台");
    expect(enUS["nav.workbench"]).toBe("Workbench");
  });
});
