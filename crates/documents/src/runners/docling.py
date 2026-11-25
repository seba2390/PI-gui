
import importlib.util
from pathlib import Path

from docling.datamodel.base_models import InputFormat
from docling.datamodel.pipeline_options import PdfPipelineOptions, RapidOcrOptions
from docling.document_converter import DocumentConverter, PdfFormatOption

# Text recognition with the models that come with rapidocr.
ocr = Path(importlib.util.find_spec("rapidocr").origin).parent / "models"
options = PdfPipelineOptions(
    artifacts_path=models,
    ocr_options=RapidOcrOptions(
        backend="onnxruntime",
        det_model_path=str(ocr / "PP-OCRv6_det_small.onnx"),
        rec_model_path=str(ocr / "PP-OCRv6_rec_small.onnx"),
        cls_model_path=str(ocr / "ch_ppocr_mobile_v2.0_cls_mobile.onnx"),
    ),
)
converter = DocumentConverter(
    allowed_formats=[InputFormat.PDF, InputFormat.DOCX, InputFormat.PPTX, InputFormat.XLSX],
    format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=options)},
)
document = converter.convert(source).document
with open(target, "w", encoding="utf-8") as out:
    out.write(document.export_to_markdown())
