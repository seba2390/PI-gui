
import pymupdf4llm

markdown = pymupdf4llm.to_markdown(source, show_progress=False)
with open(target, "w", encoding="utf-8") as out:
    out.write(markdown)
