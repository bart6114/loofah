import type { EarlyValidatorFn } from "~/store/zustand/ai-task/shared/validate";

export function createEnhanceValidator(customPrompt = false): EarlyValidatorFn {
  return (text) => {
    if (customPrompt) return { valid: true };
    const heading = text.indexOf("#");
    const output = heading > 0 ? text.slice(heading) : text;
    return output.trim().startsWith("# ")
      ? { valid: true }
      : {
          valid: false,
          feedback: "Output must start with a markdown h1 heading (# Title).",
        };
  };
}
