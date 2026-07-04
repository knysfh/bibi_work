import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const sourceRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

describe("frontend architecture boundaries", () => {
  it("does not call fetch or Tauri invoke directly from components and app screens", () => {
    const files = collectFiles(sourceRoot).filter(
      (file) =>
        file.endsWith(".tsx") &&
        (file.includes("/components/") || file.includes("/screens/") || file.includes("/app/"))
    );
    const violations = files
      .map((file) => ({
        file,
        text: readFileSync(file, "utf8")
      }))
      .filter(({ text }) => /\bfetch\s*\(|\binvoke\s*\(/.test(text))
      .map(({ file }) => file.replace(`${sourceRoot}/`, ""));

    expect(violations).toEqual([]);
  });
});

function collectFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    const stat = statSync(path);
    if (stat.isDirectory()) {
      if (entry === "node_modules" || entry === "dist") {
        return [];
      }
      return collectFiles(path);
    }
    return [path];
  });
}
