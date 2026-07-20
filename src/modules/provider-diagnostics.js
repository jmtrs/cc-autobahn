function text(value, fallback = "—") {
  return value == null || value === "" ? fallback : String(value);
}

export function diagnosticLines(snapshot) {
  if (!Array.isArray(snapshot)) return [];
  return snapshot.flatMap((provider) => {
    const heading = `${text(provider.provider).toUpperCase()} · ${text(provider.compatibility).toUpperCase()}`;
    const runtime = [provider.runtimeVersion, provider.runtimeExecutable]
      .filter(Boolean)
      .join(" · ") || "external runtime · version not reported";
    const lines = [
      { kind: "provider", label: heading, value: text(provider.surface) },
      { kind: "runtime", label: "RUNTIME", value: runtime },
    ];
    (provider.relatedRuntimes ?? []).forEach((related) => {
      const identity = [
        related.productVersion ? `product ${related.productVersion}` : null,
        related.runtimeVersion,
        related.runtimeExecutable,
      ]
        .filter(Boolean)
        .join(" · ");
      lines.push({
        kind: "runtime",
        label: text(related.surface).toUpperCase(),
        value: identity || "installed runtime · version not reported",
      });
    });
    (provider.capabilities ?? []).forEach((capability) => {
      const status = text(capability.status).toUpperCase();
      const quality = text(capability.quality).toUpperCase();
      const source = text(capability.source);
      lines.push({
        kind: "capability",
        label: text(capability.id).replaceAll("-", " ").toUpperCase(),
        value: `${status} · ${quality} · ${source}`,
      });
      if (capability.status !== "available") {
        const detail = [
          capability.reason,
          capability.fallback ? `fallback: ${capability.fallback}` : null,
          capability.remediation,
        ]
          .filter(Boolean)
          .join(" · ");
        if (detail) lines.push({ kind: "detail", label: "", value: detail });
      }
    });
    return lines;
  });
}

export function createLatestRequestGate() {
  let generation = 0;
  return {
    begin() {
      generation += 1;
      return generation;
    },
    cancel() {
      generation += 1;
    },
    isCurrent(candidate) {
      return candidate === generation;
    },
  };
}

export function renderDiagnostics(snapshot, root) {
  root.replaceChildren();
  diagnosticLines(snapshot).forEach((line) => {
    const row = document.createElement("div");
    row.className = `diagnostics-row diagnostics-${line.kind}`;
    const label = document.createElement("span");
    label.className = "diagnostics-label";
    label.textContent = line.label;
    const value = document.createElement("span");
    value.className = "diagnostics-value";
    value.textContent = line.value;
    row.append(label, value);
    root.appendChild(row);
  });
}

export async function wireProviderDiagnostics() {
  const button = document.getElementById("diagnostics-settings-btn");
  const overlay = document.getElementById("diagnostics-overlay");
  const body = document.getElementById("diagnostics-body");
  const requests = createLatestRequestGate();
  document.getElementById("diagnostics-close").onclick = () => {
    requests.cancel();
    overlay.hidden = true;
  };
  button.onclick = async () => {
    const generation = requests.begin();
    overlay.hidden = false;
    body.textContent = "loading…";
    if (!("__TAURI_INTERNALS__" in window)) {
      if (requests.isCurrent(generation)) {
        body.textContent = "Diagnostics require the native app runtime.";
      }
      return;
    }
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const snapshot = await invoke("provider_diagnostics_snapshot");
      if (requests.isCurrent(generation)) renderDiagnostics(snapshot, body);
    } catch (error) {
      if (requests.isCurrent(generation)) {
        body.textContent = `Diagnostics unavailable: ${error}`;
      }
    }
  };
}
