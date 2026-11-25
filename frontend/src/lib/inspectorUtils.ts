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
  citationKeyOverride?: string;
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
      if (field === 'tags' || field === 'exclude_from_cloud_llm' || field === 'citation_key_override') {
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
      citation_key_override: optTrimmedStr(fields.citationKeyOverride),
      exclude_from_cloud_llm: fields.excludeFromCloud,
      tags: fields.tagsText
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean),
      edited_fields: fields.touched,
    },
  };
}

type ClearableStringKey =
  | 'authorsText'
  | 'journalAbbrev'
  | 'journalFull'
  | 'booktitle'
  | 'series'
  | 'volume'
  | 'issueNumber'
  | 'pages'
  | 'edition'
  | 'publisher'
  | 'address'
  | 'doi'
  | 'arxivId'
  | 'mrNumber'
  | 'mrReviewer'
  | 'zblNumber'
  | 'isbn'
  | 'issn'
  | 'url'
  | 'mscPrimary'
  | 'mscSecondaryText'
  | 'abstractText';

const CLEARABLE_FIELD_MAP: ReadonlyArray<{ lockKey: string; prop: ClearableStringKey | 'yearStr' }> = [
  { lockKey: 'authors', prop: 'authorsText' },
  { lockKey: 'year', prop: 'yearStr' },
  { lockKey: 'pages', prop: 'pages' },
  { lockKey: 'journal_abbrev', prop: 'journalAbbrev' },
  { lockKey: 'journal_full', prop: 'journalFull' },
  { lockKey: 'booktitle', prop: 'booktitle' },
  { lockKey: 'series', prop: 'series' },
  { lockKey: 'volume', prop: 'volume' },
  { lockKey: 'issue_number', prop: 'issueNumber' },
  { lockKey: 'edition', prop: 'edition' },
  { lockKey: 'publisher', prop: 'publisher' },
  { lockKey: 'address', prop: 'address' },
  { lockKey: 'doi', prop: 'doi' },
  { lockKey: 'arxiv_id', prop: 'arxivId' },
  { lockKey: 'mr_number', prop: 'mrNumber' },
  { lockKey: 'mr_reviewer', prop: 'mrReviewer' },
  { lockKey: 'zbl_number', prop: 'zblNumber' },
  { lockKey: 'isbn', prop: 'isbn' },
  { lockKey: 'issn', prop: 'issn' },
  { lockKey: 'url', prop: 'url' },
  { lockKey: 'msc_primary', prop: 'mscPrimary' },
  { lockKey: 'msc_secondary', prop: 'mscSecondaryText' },
  { lockKey: 'abstract_text', prop: 'abstractText' },
];

export function hasNonEmptyUnlockedFields(
  fields: InspectorFormFields,
  lockedFields: string[]
): boolean {
  for (const { lockKey, prop } of CLEARABLE_FIELD_MAP) {
    if (lockedFields.includes(lockKey)) continue;
    if (normalizeInputText(fields[prop]) !== '') {
      return true;
    }
  }
  return false;
}

/**
 * Empties every unlocked bibliographic field (keeping `work_type`, `title`, locked fields 🔒,
 * tags, and cloud-exclusion untouched) so wrong fetched metadata can be wiped in one click.
 */
export function clearUnlockedFields(
  fields: InspectorFormFields,
  lockedFields: string[]
): { next: InspectorFormFields; clearedFields: string[] } {
  const next: InspectorFormFields = { ...fields, touched: [...fields.touched] };
  const clearedFields: string[] = [];
  for (const { lockKey, prop } of CLEARABLE_FIELD_MAP) {
    if (lockedFields.includes(lockKey)) continue;
    if (normalizeInputText(next[prop]) !== '') {
      next[prop] = '';
      clearedFields.push(lockKey);
      if (!next.touched.includes(lockKey)) {
        next.touched.push(lockKey);
      }
    }
  }
  return { next, clearedFields };
}

export function hasInspectorFormDiff(
  meta: DocumentMetadataView,
  fields: InspectorFormFields,
  locksModified: boolean
): boolean {
  if (fields.touched.length > 0 || locksModified) return true;
  if (fields.workType !== (meta.work_type || 'article')) return true;
  if (normalizeInputText(fields.title) !== (meta.title ?? '').trim()) return true;
  if (normalizeInputText(fields.authorsText) !== formatAuthorsForInput(meta.authors).trim()) {
    return true;
  }
  const metaYearStr = meta.year !== null && meta.year !== undefined ? String(meta.year) : '';
  if (normalizeInputText(fields.yearStr) !== metaYearStr) return true;
  if (normalizeInputText(fields.pages) !== (meta.pages ?? '').trim()) return true;
  if (normalizeInputText(fields.journalAbbrev) !== (meta.journal_abbrev ?? '').trim()) return true;
  if (normalizeInputText(fields.journalFull) !== (meta.journal_full ?? '').trim()) return true;
  if (normalizeInputText(fields.booktitle) !== (meta.booktitle ?? '').trim()) return true;
  if (normalizeInputText(fields.series) !== (meta.series ?? '').trim()) return true;
  if (normalizeInputText(fields.volume) !== (meta.volume ?? '').trim()) return true;
  if (normalizeInputText(fields.issueNumber) !== (meta.issue_number ?? '').trim()) return true;
  if (normalizeInputText(fields.edition) !== (meta.edition ?? '').trim()) return true;
  if (normalizeInputText(fields.publisher) !== (meta.publisher ?? '').trim()) return true;
  if (normalizeInputText(fields.address) !== (meta.address ?? '').trim()) return true;
  if (normalizeInputText(fields.doi) !== (meta.doi ?? '').trim()) return true;
  if (normalizeInputText(fields.arxivId) !== (meta.arxiv_id ?? '').trim()) return true;
  if (normalizeInputText(fields.mrNumber) !== (meta.mr_number ?? '').trim()) return true;
  if (normalizeInputText(fields.mrReviewer) !== (meta.mr_reviewer ?? '').trim()) return true;
  if (normalizeInputText(fields.zblNumber) !== (meta.zbl_number ?? '').trim()) return true;
  if (normalizeInputText(fields.isbn) !== (meta.isbn ?? '').trim()) return true;
  if (normalizeInputText(fields.issn) !== (meta.issn ?? '').trim()) return true;
  if (normalizeInputText(fields.url) !== (meta.url ?? '').trim()) return true;
  if (normalizeInputText(fields.mscPrimary) !== (meta.msc_primary ?? '').trim()) return true;
  if (normalizeInputText(fields.mscSecondaryText) !== meta.msc_secondary.join(', ').trim()) {
    return true;
  }
  if (normalizeInputText(fields.abstractText) !== (meta.abstract_text ?? '').trim()) return true;
  if (
    normalizeInputText(fields.citationKeyOverride) !== (meta.citation_key_override ?? '').trim()
  ) {
    return true;
  }
  if (normalizeInputText(fields.tagsText) !== meta.tags.join(', ').trim()) return true;
  if (fields.excludeFromCloud !== meta.exclude_from_cloud_llm) return true;
  return false;
}
