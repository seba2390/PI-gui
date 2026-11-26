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
        .map((tool: any) => {
          const schema = tool.parameters ?? {};
          const required: string[] = schema.required ?? [];
          return {
            name: tool.name,
            description: tool.description ?? "",
            guidelines: tool.promptGuidelines ?? [],
            // The parameters' JSON schema, as the model gets it.
            schema: JSON.stringify(schema),
            parameters: Object.entries(schema.properties ?? {}).map(([name, property]: [string, any]) => ({
              name,
              required: required.includes(name),
              type: typeof property?.type === "string" ? property.type : "",
              description: property?.description ?? "",
            })),
            path: tool.sourceInfo?.path ?? "",
            active: active.has(tool.name),
          };
        });
      ctx?.ui?.setStatus?.(KEY, JSON.stringify(tools));
    }, 300);
  });
}
