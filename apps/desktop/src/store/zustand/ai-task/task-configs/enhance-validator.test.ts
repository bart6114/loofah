import { describe, expect, it } from "vitest";

import { createEnhanceValidator } from "./enhance-validator";

describe("summary formatting validation", () => {
  it("requires an h1 for the default prompt", () => {
    const validate = createEnhanceValidator();
    expect(validate("# Decisions\n- Ship Friday")).toEqual({ valid: true });
    expect(validate("Preamble\n# Decisions")).toEqual({ valid: true });
    expect(validate("## Decisions")).toMatchObject({ valid: false });
    expect(validate("A paragraph")).toMatchObject({ valid: false });
  });
  it("allows customized prompts to choose their own format", () => {
    const validate = createEnhanceValidator(true);
    for (const text of [
      "A paragraph",
      "## Decisions",
      "- Ship Friday",
      '{"decision":"ship"}',
    ]) {
      expect(validate(text)).toEqual({ valid: true });
    }
  });
});
