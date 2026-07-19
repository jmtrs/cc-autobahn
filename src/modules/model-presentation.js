const CLAUDE_SLOTS = Object.freeze([
  { key: "opus", code: "O", label: "Opus" },
  { key: "sonnet", code: "S", label: "Sonnet" },
  { key: "haiku", code: "H", label: "Haiku" },
  { key: "fable", code: "F", label: "Fable" },
  { key: "custom", code: "C", label: "Custom Claude-compatible model" },
]);

const CODEX_SLOTS = Object.freeze([
  { key: "gpt", code: "G", label: "GPT" },
  { key: "sol", code: "S", label: "Sol" },
  { key: "terra", code: "T", label: "Terra" },
  { key: "mini", code: "M", label: "Mini" },
  { key: "custom", code: "C", label: "Custom Codex model" },
]);

const CLAUDE_NAMEPLATES = Object.freeze({
  opus: "CC 500",
  sonnet: "CC 320",
  haiku: "CC 220 CDI",
  fable: "CC 63 AMG",
});

export function modelSlots(provider) {
  return provider === "codex" ? CODEX_SLOTS : CLAUDE_SLOTS;
}

function normalizedModelId(modelId) {
  return String(modelId ?? "").trim().toLowerCase();
}

function compactFallback(modelId) {
  return normalizedModelId(modelId).replace(/[^a-z0-9]/g, "").slice(0, 4).toUpperCase() || "?";
}

function codexLabel(id) {
  if (!id) return "?";
  return id
    .replace(/^openai[\/:_-]+/, "")
    .split(/[-_/]+/)
    .filter(Boolean)
    .map((part) => (part === "gpt" ? "GPT" : part.toUpperCase()))
    .join(" ");
}

function codexCode(id, slotKey) {
  const version = id.match(/gpt[-_]?([0-9]+(?:\.[0-9]+)?)/)?.[1];
  if (version) {
    const suffix =
      slotKey === "sol" ? "S" : slotKey === "terra" ? "T" : slotKey === "mini" ? "M" : "";
    return `${version}${suffix}`.slice(0, 4).toUpperCase();
  }
  if (slotKey === "mini") return "MINI";
  return compactFallback(id);
}

export function resolveModelPresentation(provider, modelId) {
  const id = normalizedModelId(modelId);
  if (!id) return null;

  if (provider === "codex") {
    const slotKey = id.includes("terra")
      ? "terra"
      : id.includes("sol")
        ? "sol"
        : id.includes("mini")
          ? "mini"
          : id.includes("gpt")
            ? "gpt"
            : "custom";
    return {
      modelKey: id,
      slotKey,
      nameplate: codexLabel(id),
      code: codexCode(id, slotKey),
      editable: slotKey !== "custom",
    };
  }

  const family = ["opus", "sonnet", "haiku", "fable"].find((key) => id.includes(key));
  if (family) {
    return {
      modelKey: family,
      slotKey: family,
      nameplate: CLAUDE_NAMEPLATES[family],
      code: modelSlots("claude").find((slot) => slot.key === family).code,
      editable: true,
    };
  }
  return {
    modelKey: id,
    slotKey: "custom",
    nameplate: compactFallback(id),
    code: compactFallback(id),
    editable: false,
  };
}

export function defaultNameplate(provider, modelKey) {
  if (provider === "claude") return CLAUDE_NAMEPLATES[modelKey] ?? null;
  return resolveModelPresentation(provider, modelKey)?.nameplate ?? null;
}

export function formatProviderModelCode(provider, modelId) {
  return resolveModelPresentation(provider, modelId)?.code ?? "?";
}
