# Document parsing

How documents are read: the ones you attach to a chat, PDF files that chats find on the web,
and those that pi reads in Code sessions on this computer. Each is turned into Markdown for the
model.

## Parser

```{image} /_static/shots/documents-parser-light.webp
:class: shot only-light
:alt: The parsers
```

```{image} /_static/shots/documents-parser-dark.webp
:class: shot only-dark
:alt: The parsers
```

The built-in reader is the default and needs no download. The others are downloaded on request
and can be removed again (macOS and Linux):

| Parser | Good at | Download (on disk) | License |
|---|---|---|---|
| Built-in | Plain text of PDFs, page by page; Word, PowerPoint, Excel, OpenDocument and EPUB files | none | MIT |
| [PyMuPDF4LLM](https://github.com/pymupdf/pymupdf) | Fast; headings, columns and tables of PDFs made on a computer | 143 MB (320 MB) | AGPL-3.0 |
| [Docling](https://github.com/docling-project/docling) | Complex layouts, tables, scans; also Word, PowerPoint and Excel | 788 MB (1.8 GB); Linux 3.7 GB (8.5 GB) | MIT |
| [MinerU](https://github.com/opendatalab/MinerU) | Academic papers: formulas, dense columns, long tables, scans | 1.0 GB (1.3 GB) | MinerU Open Source License |

Sizes are for a Mac. On Linux, Docling includes NVIDIA's libraries. Macs with an Intel processor
only have PyMuPDF4LLM; parsers need macOS 14 or newer, and Windows has only the built-in reader.
The page says what isn't available.

Downloading asks first and shows the parts, their sizes and the license. A private Python, the
packages and the models are each pinned and checked against their checksum, and go to
`~/.pi/agent/pistachio/documents/`; your own Python is not touched. Parsers run one process per
document, without network, for at most 10 minutes (2 for a PDF from the web). If the chosen
parser is missing, fails or takes too long, the built-in reader is used and the result says so.

## Long files in chats

```{image} /_static/shots/documents-long-files-light.webp
:class: shot only-light
:alt: The Long files in chats section
```

```{image} /_static/shots/documents-long-files-dark.webp
:class: shot only-dark
:alt: The Long files in chats section
```

Send whole up to
: Text longer than this (256 KB by default, from 32 KB to all that fits) goes to the model
  shortened: its start, its outline and the parts that match your message. The attachment card
  says "Shortened", and the model is told. Text that doesn't fit in what is left of the model's
  context is always shortened. Everything sent counts again with each later message: 256 KB is
  about 65,000 tokens.

## Where documents are read

Chats
: Attach PDF, Word, PowerPoint, Excel (`.docx`, `.pptx`, `.xlsx`, `.xls`, `.xlsb`),
  OpenDocument (`.odt`, `.odp`, `.ods`), EPUB or plain-text files. The card shows "Reading…"
  until the text is ready. The text goes to the model with the message but isn't shown. Edit and
  Retry keep it.

Web search
: PDFs found on the web are read with the same parser.

Code sessions
: An attached PDF stays an `@path`. When pi reads a PDF on this computer, it gets the Markdown.

## Viewers

Click an attached file to open it in the app.

- **PDF viewer**: pages drawn as you scroll, light or dark pages (the sun or moon button), links,
  zoom with the buttons, ⌘+ / ⌘- / ⌘0 or a pinch. Another button opens the file in your usual app.
- **Text viewer**: line numbers and syntax colors, selection, copy and ⌘F; a button wraps long
  lines.

To try a parser without the app: `pistachio --documents-tool parse paper.pdf` prints the
Markdown.
