
# Before MinerU is imported: it reads these once.
os.environ.update(
    MINERU_HOME=os.environ["HOME"],
    MINERU_MODEL_BASE_DIR=models,
    MINERU_MODEL_SOURCE="local",
    MINERU_MODEL_SMALL_BACKEND="onnx",
    MINERU_TELEMETRY_DISABLED="1",
)

from mineru.parser import parse

# "basic": the small models, without the language model that needs a strong GPU.
result = parse(source, tier="basic", image_analysis=False)
with open(target, "w", encoding="utf-8") as out:
    out.write(result.markdown())
