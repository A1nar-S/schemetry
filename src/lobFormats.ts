// Content detection for the LOB viewer: works out which views a value supports and
// which one to open first, from magic bytes and a light look at the text.

export type LobTab = 'text' | 'formatted' | 'html' | 'image' | 'pdf' | 'word' | 'hex';

export type LobFormat = {
  /** Short label for the header, e.g. "PNG image", "JSON". */
  label: string;
  /** MIME type used for previews and the save-file extension. */
  mime: string;
  /** Views that make sense for this content, in tab order. */
  tabs: LobTab[];
  /** The view to open first. */
  initial: LobTab;
  /** Pretty-printed JSON/XML, when the text parses as one. */
  formatted: string | null;
  /** CodeMirror language for the text views. */
  language: 'json' | 'xml' | 'html' | null;
};

const IMAGE_SIGNATURES: [number[], string, string][] = [
  [[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], 'image/png', 'PNG image'],
  [[0xff, 0xd8, 0xff], 'image/jpeg', 'JPEG image'],
  [[0x47, 0x49, 0x46, 0x38], 'image/gif', 'GIF image'],
  [[0x42, 0x4d], 'image/bmp', 'BMP image'],
];

// Office Open XML files are zips; the part names in the central directory (at the
// end of the file) tell them apart.
const OOXML: [string, string, string][] = [
  ['word/', 'application/vnd.openxmlformats-officedocument.wordprocessingml.document', 'Word document'],
  ['xl/', 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', 'Excel workbook'],
  ['ppt/', 'application/vnd.openxmlformats-officedocument.presentationml.presentation', 'PowerPoint presentation'],
];

const EXTENSIONS: Record<string, string> = {
  'application/pdf': '.pdf',
  'image/png': '.png',
  'image/jpeg': '.jpg',
  'image/gif': '.gif',
  'image/webp': '.webp',
  'image/bmp': '.bmp',
  'image/svg+xml': '.svg',
  'application/zip': '.zip',
  'application/gzip': '.gz',
  'application/rtf': '.rtf',
  'application/msword': '.doc',
  'application/json': '.json',
  'application/xml': '.xml',
  'text/html': '.html',
  'text/plain': '.txt',
  [OOXML[0][1]]: '.docx',
  [OOXML[1][1]]: '.xlsx',
  [OOXML[2][1]]: '.pptx',
};

export function extForMime(mime: string): string {
  return EXTENSIONS[mime] ?? '.bin';
}

function startsWith(bytes: Uint8Array, sig: number[], offset = 0): boolean {
  if (bytes.length < offset + sig.length) return false;
  return sig.every((b, i) => bytes[offset + i] === b);
}

function ascii(bytes: Uint8Array, start: number, end: number): string {
  let s = '';
  for (let i = start; i < end; i++) s += String.fromCharCode(bytes[i]);
  return s;
}

/** Decodes UTF-8, or returns null when the bytes aren't text (invalid UTF-8 or NULs). */
export function decodeUtf8(bytes: Uint8Array): string | null {
  try {
    const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    return text.includes('\0') ? null : text;
  } catch {
    return null;
  }
}

// Above this, pretty-printing would freeze the UI for little benefit.
const FORMAT_LIMIT = 5 * 1024 * 1024;

function prettyJson(text: string): string | null {
  const t = text.trimStart();
  if (!(t.startsWith('{') || t.startsWith('[')) || text.length > FORMAT_LIMIT) return null;
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return null;
  }
}

function prettyXml(text: string): string | null {
  if (!text.trimStart().startsWith('<') || text.length > FORMAT_LIMIT) return null;
  const doc = new DOMParser().parseFromString(text, 'application/xml');
  if (doc.getElementsByTagName('parsererror').length) return null;
  const lines: string[] = [];
  const walk = (node: Node, depth: number) => {
    const pad = '  '.repeat(depth);
    if (node.nodeType === Node.ELEMENT_NODE) {
      const el = node as Element;
      const attrs = [...el.attributes].map(a => ` ${a.name}="${escapeXml(a.value)}"`).join('');
      const children = [...el.childNodes].filter(
        c => c.nodeType !== Node.TEXT_NODE || (c.textContent ?? '').trim() !== '',
      );
      if (!children.length) {
        lines.push(`${pad}<${el.tagName}${attrs}/>`);
      } else if (children.length === 1 && children[0].nodeType === Node.TEXT_NODE) {
        lines.push(`${pad}<${el.tagName}${attrs}>${escapeXml(children[0].textContent!.trim())}</${el.tagName}>`);
      } else {
        lines.push(`${pad}<${el.tagName}${attrs}>`);
        children.forEach(c => walk(c, depth + 1));
        lines.push(`${pad}</${el.tagName}>`);
      }
    } else if (node.nodeType === Node.TEXT_NODE) {
      lines.push(pad + escapeXml(node.textContent!.trim()));
    } else if (node.nodeType === Node.CDATA_SECTION_NODE) {
      lines.push(`${pad}<![CDATA[${node.textContent}]]>`);
    } else if (node.nodeType === Node.COMMENT_NODE) {
      lines.push(`${pad}<!--${node.textContent}-->`);
    } else if (node.nodeType === Node.PROCESSING_INSTRUCTION_NODE) {
      const pi = node as ProcessingInstruction;
      lines.push(`${pad}<?${pi.target} ${pi.data}?>`);
    }
  };
  const decl = text.trimStart().match(/^<\?xml[^?]*\?>/);
  if (decl) lines.push(decl[0]);
  doc.childNodes.forEach(n => walk(n, 0));
  return lines.join('\n');
}

function escapeXml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function looksLikeHtml(text: string): boolean {
  const head = text.trimStart().slice(0, 2048).toLowerCase();
  return /^<(!doctype html|html|head|body)\b/.test(head) || /<(p|div|table|span|br|h[1-6]|ul|a)\b[^>]*>/.test(head);
}

/** Detects the format of text content (CLOBs, plain cells, or BLOBs that decode as text). */
function detectText(text: string): LobFormat {
  const trimmed = text.trimStart();
  const json = prettyJson(text);
  if (json !== null) {
    return { label: 'JSON', mime: 'application/json', tabs: ['text', 'formatted', 'hex'], initial: 'formatted', formatted: json, language: 'json' };
  }
  if (/^(<\?xml[^>]*>\s*)?(<!--[\s\S]*?-->\s*)*<svg\b/i.test(trimmed)) {
    return { label: 'SVG image', mime: 'image/svg+xml', tabs: ['image', 'text', 'formatted', 'hex'], initial: 'image', formatted: prettyXml(text), language: 'xml' };
  }
  if (looksLikeHtml(text)) {
    return { label: 'HTML', mime: 'text/html', tabs: ['html', 'text', 'hex'], initial: 'html', formatted: null, language: 'html' };
  }
  const xml = prettyXml(text);
  if (xml !== null) {
    return { label: 'XML', mime: 'application/xml', tabs: ['text', 'formatted', 'html', 'hex'], initial: 'formatted', formatted: xml, language: 'xml' };
  }
  if (trimmed.startsWith('{\\rtf')) {
    return { label: 'RTF document', mime: 'application/rtf', tabs: ['text', 'hex'], initial: 'text', formatted: null, language: null };
  }
  return { label: 'Text', mime: 'text/plain', tabs: ['text', 'hex'], initial: 'text', formatted: null, language: null };
}

/**
 * Detects the format of binary content. `complete` is false when only a prefix of the
 * value was loaded — previews that need the whole file (images, PDF, Word) are then
 * left out, since a cut-off file wouldn't render.
 */
function detectBinary(bytes: Uint8Array, complete: boolean): LobFormat {
  const binary = (label: string, mime: string, preview: LobTab | null): LobFormat => ({
    label,
    mime,
    tabs: preview && complete ? [preview, 'hex'] : ['hex'],
    initial: preview && complete ? preview : 'hex',
    formatted: null,
    language: null,
  });

  if (startsWith(bytes, [0x25, 0x50, 0x44, 0x46])) return binary('PDF document', 'application/pdf', 'pdf');
  for (const [sig, mime, label] of IMAGE_SIGNATURES) {
    if (startsWith(bytes, sig)) return binary(label, mime, 'image');
  }
  if (startsWith(bytes, [0x52, 0x49, 0x46, 0x46]) && startsWith(bytes, [0x57, 0x45, 0x42, 0x50], 8)) {
    return binary('WebP image', 'image/webp', 'image');
  }
  if (startsWith(bytes, [0x50, 0x4b, 0x03, 0x04])) {
    const tail = ascii(bytes, Math.max(0, bytes.length - 256 * 1024), bytes.length);
    const office = OOXML.find(([prefix]) => tail.includes(prefix));
    if (office) return binary(office[2], office[1], office[0] === 'word/' ? 'word' : null);
    return binary('ZIP archive', 'application/zip', null);
  }
  if (startsWith(bytes, [0x1f, 0x8b])) return binary('GZIP archive', 'application/gzip', null);
  if (startsWith(bytes, [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1])) {
    return binary('Legacy Office document (.doc/.xls)', 'application/msword', null);
  }

  const text = decodeUtf8(bytes);
  if (text !== null) {
    const format = detectText(text);
    // A cut-off SVG won't render either.
    return complete ? format : { ...format, tabs: format.tabs.filter(t => t !== 'image' && t !== 'html') };
  }
  return binary('Binary data', 'application/octet-stream', null);
}

export function detectFormat(content: { text: string } | { bytes: Uint8Array; complete: boolean }): LobFormat {
  return 'text' in content ? detectText(content.text) : detectBinary(content.bytes, content.complete);
}
