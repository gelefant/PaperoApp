import type {
  BibliographicAuthor,
  DocumentMetadataView,
  ManualMetadataEdit,
} from './types';

export type YearParseResult =
  | { ok: true; year: number | null }
  | { ok: false; error: string };

export interface InspectorFormFields {
  workType: string;
  title: string;
  authorsText: string;
  yearStr: string | number | null | undefined;
  journalAbbrev: string;
  journalFull: string;
  booktitle: string;
  series: string;
  volume: string;
  issueNumber: string;
  pages: string;
  edition: string;
  publisher: string;
  address: string;
  doi: string;
  arxivId: string;
  mrNumber: string;
  mrReviewer: string;
  zblNumber: string;
  isbn: string;
  issn: string;
  url: string;
  mscPrimary: string;
  mscSecondaryText: string;
  abstractText: string;
  tagsText: string;
  excludeFromCloud: boolean;
  touched: string[];
}

/**
 * Normalizes any input value (including `null` or `number` coerced by Svelte `<input type="number">`)
 * into a trimmed string without throwing.
 */
export function normalizeInputText(val: unknown): string {
  if (val === null || val === undefined) return '';
  return String(val).trim();
}

export function optTrimmedStr(val: unknown): string | null {
  const t = normalizeInputText(val);
  return t ? t : null;
}

export function parseYearInput(raw: unknown): YearParseResult {
  const cleaned = normalizeInputText(raw);
  if (!cleaned) {
    return { ok: true, year: null };
  }
  if (!/^\d{1,4}$/.test(cleaned)) {
    return {
      ok: false,
      error: `Anno non valido («${cleaned}»): inserisci un numero intero (es. 2002) oppure lascia vuoto.`,
    };
  }
  const num = Number(cleaned);
  if (!Number.isInteger(num) || num <= 0) {
    return {
      ok: false,
      error: `Anno non valido («${cleaned}»): inserisci un anno positivo oppure lascia vuoto.`,
    };
  }
  return { ok: true, year: num };
}

export function formatAuthorsForInput(authors: BibliographicAuthor[]): string {
  return authors
    .map((a) => (a.given_name.trim() ? `${a.family_name}, ${a.given_name}` : a.family_name))
    .join('; ');
}

export function parseAuthorsInput(raw: string): BibliographicAuthor[] {
  return raw
    .split(';')
    .map((s) => s.trim())
    .filter(Boolean)
    .map((part) => {
      const idx = part.indexOf(',');
      if (idx >= 0) {
        return {
          family_name: part.slice(0, idx).trim(),
          given_name: part.slice(idx + 1).trim(),
          role: 'author',
        };
      }
      const tokens = part.split(/\s+/);
      if (tokens.length >= 2) {
        return {
          family_name: tokens[tokens.length - 1],
          given_name: tokens.slice(0, -1).join(' '),
          role: 'author',
        };
      }
      return { family_name: part, given_name: '', role: 'author' };
    });
}

export function computeNextLocksOnTouch(
  lockedFields: string[],
  field: string,
  overwriteQueryFields: boolean,
  currentValue?: string | number | null
): { lockedFields: string[]; modified: boolean } {
  if (field === 'tags' || field === 'exclude_from_cloud_llm') {
    return { lockedFields, modified: false };
  }
  if ((field === 'title' || field === 'authors') && overwriteQueryFields) {
    return { lockedFields, modified: false };
  }
  if (currentValue !== undefined) {
    const trimmed = normalizeInputText(currentValue);
    if (trimmed) {
      if (!lockedFields.includes(field)) {
        return { lockedFields: [...lockedFields, field], modified: true };
      }
      return { lockedFields, modified: true };
    }
    return {
      lockedFields: lockedFields.filter((f) => f !== field),
      modified: true,
    };
  }
  if (!lockedFields.includes(field)) {
    return { lockedFields: [...lockedFields, field], modified: true };
  }
  return { lockedFields, modified: false };
}

export function buildManualMetadataEditPayload(
  meta: DocumentMetadataView,
  fields: InspectorFormFields,
  allowEmptyTitle = false
): { ok: true; payload: ManualMetadataEdit } | { ok: false; error: string } {
  const yearRes = parseYearInput(fields.yearStr);
  if (!yearRes.ok) {
    return yearRes;
  }
  const trimmedTitle = normalizeInputText(fields.title);
  return {
    ok: true,
    payload: {
      document_id: meta.document_id,
      work_type: fields.workType,
      title: allowEmptyTitle ? trimmedTitle : trimmedTitle || meta.title,
      authors: parseAuthorsInput(fields.authorsText),
      journal_abbrev: optTrimmedStr(fields.journalAbbrev),
      journal_full: optTrimmedStr(fields.journalFull),
      booktitle: optTrimmedStr(fields.booktitle),
      series: optTrimmedStr(fields.series),
      volume: optTrimmedStr(fields.volume),
      issue_number: optTrimmedStr(fields.issueNumber),
      year: yearRes.year,
      pages: optTrimmedStr(fields.pages),
      edition: optTrimmedStr(fields.edition),
      publisher: optTrimmedStr(fields.publisher),
      address: optTrimmedStr(fields.address),
      isbn: optTrimmedStr(fields.isbn),
      issn: optTrimmedStr(fields.issn),
      doi: optTrimmedStr(fields.doi),
      arxiv_id: optTrimmedStr(fields.arxivId),
      mr_number: optTrimmedStr(fields.mrNumber),
      mr_reviewer: optTrimmedStr(fields.mrReviewer),
      zbl_number: optTrimmedStr(fields.zblNumber),
      url: optTrimmedStr(fields.url),
      msc_primary: optTrimmedStr(fields.mscPrimary),
      msc_secondary: fields.mscSecondaryText
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean),
      abstract_text: optTrimmedStr(fields.abstractText),
      exclude_from_cloud_llm: fields.excludeFromCloud,
      tags: fields.tagsText
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean),
      edited_fields: fields.touched,
    },
  };
}
