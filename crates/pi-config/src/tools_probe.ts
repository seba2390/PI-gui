// Pistachio's tool probe: reports the tools pi has, as it describes them to the model, through a
// status line, then pi is stopped. Loaded only into a pi that keeps no session (Settings → Tools).
const KEY = "pistachio-tools";

export default function (pi: any) {
  pi.on("session_start", async (_event: unknown, ctx: any) => {
    // After the other extensions have set which tools are active.
    setTimeout(() => {
      const active = new Set(pi.getActiveTools());
      const tools = pi
        .getAllTools()
        .filter((tool: any) => tool.exposure !== "hidden")
        .map((tool: any) => ({
          name: tool.name,
          description: tool.description ?? "",
          guidelines: tool.promptGuidelines ?? [],
          parameters: Object.keys(tool.parameters?.properties ?? {}).map((name) => ({
            name,
            required: (tool.parameters?.required ?? []).includes(name),
          })),
          path: tool.sourceInfo?.path ?? "",
          active: active.has(tool.name),
        }));
      ctx?.ui?.setStatus?.(KEY, JSON.stringify(tools));
    }, 300);
  });
}
