import { describe, expect, it } from 'vitest';
import type { DocumentMetadataView } from './types';
import {
  buildManualMetadataEditPayload,
  computeNextLocksOnTouch,
  normalizeInputText,
  parseYearInput,
  type InspectorFormFields,
} from './inspectorUtils';

function makeJolliffeMeta(): DocumentMetadataView {
  return {
    document_id: 2002,
    work_type: 'book',
    title: 'Principal Component Analysis',
    authors: [{ family_name: 'Jolliffe', given_name: 'I. T.', role: 'author' }],
    journal_abbrev: 'Philos. Trans. Roy. Soc. A',
    journal_full:
      'Philosophical Transactions of the Royal Society A. Mathematical, Physical and Engineering Sciences',
    booktitle: null,
    series: null,
    volume: '374',
    issue_number: '2065',
    year: 2016,
    pages: '20150202, 16',
    edition: null,
    publisher: null,
    address: null,
    isbn: null,
    issn: '1364-503X,1471-2962',
    doi: '10.1098/rsta.2015.0202',
    arxiv_id: null,
    arxiv_version: null,
    mr_number: '3479904',
    mr_number_display: '3479904',
    mr_reviewer: null,
    zbl_number: null,
    url: 'https://doi.org/10.1098/rsta.2015.0202',
    msc_primary: '62H25',
    msc_primary_status: 'metadata',
    msc_secondary: ['62-02'],
    abstract_text: null,
    raw_mrlookup_bibtex: null,
    metadata_source: 'mrlookup',
    metadata_license_note: null,
    manual_locked_fields: ['authors', 'title', 'work_type'],
    exclude_from_cloud_llm: false,
    metadata_verified: false,
    tags: [],
    pending_candidates: [],
  };
}

function makeClearedJolliffeFields(
  yearStr: string | number | null | undefined
): InspectorFormFields {
  return {
    workType: 'book',
    title: 'Principal Component Analysis',
    authorsText: 'Jolliffe, I. T.',
    yearStr,
    journalAbbrev: '',
    journalFull: '',
    booktitle: '',
    series: '',
    volume: '',
    issueNumber: '',
    pages: '',
    edition: '',
    publisher: '',
    address: '',
    doi: '',
    arxivId: '',
    mrNumber: '',
    mrReviewer: '',
    zblNumber: '',
    isbn: '',
    issn: '',
    url: '',
    mscPrimary: '',
    mscSecondaryText: '',
    abstractText: '',
    tagsText: '',
    excludeFromCloud: false,
    touched: [
      'year',
      'pages',
      'journal_abbrev',
      'journal_full',
      'volume',
      'issue_number',
      'doi',
      'mr_number',
      'issn',
      'url',
      'msc_primary',
      'msc_secondary',
    ],
  };
}

describe('inspectorUtils', () => {
  it('normalizes string, number, null, and undefined safely without throwing', () => {
    expect(normalizeInputText(' 2002 ')).toBe('2002');
    expect(normalizeInputText(2002)).toBe('2002');
    expect(normalizeInputText(null)).toBe('');
    expect(normalizeInputText(undefined)).toBe('');
  });

  it('parses cleared, numeric, and string year inputs cleanly', () => {
    expect(parseYearInput('')).toEqual({ ok: true, year: null });
    expect(parseYearInput('   ')).toEqual({ ok: true, year: null });
    expect(parseYearInput(null)).toEqual({ ok: true, year: null });
    expect(parseYearInput('2002')).toEqual({ ok: true, year: 2002 });
    expect(parseYearInput(2002)).toEqual({ ok: true, year: 2002 });
    expect(parseYearInput('2002.5').ok).toBe(false);
    expect(parseYearInput('abc').ok).toBe(false);
    expect(parseYearInput('0').ok).toBe(false);
  });

  it('handles touch on year when cleared (empty or null) or set to number without crashing', () => {
    const clearedEmpty = computeNextLocksOnTouch(
      ['authors', 'title', 'work_type', 'year'],
      'year',
      false,
      ''
    );
    expect(clearedEmpty.lockedFields).toEqual(['authors', 'title', 'work_type']);
    expect(clearedEmpty.modified).toBe(true);

    const clearedNull = computeNextLocksOnTouch(
      ['authors', 'title', 'work_type', 'year'],
      'year',
      false,
      null
    );
    expect(clearedNull.lockedFields).toEqual(['authors', 'title', 'work_type']);
    expect(clearedNull.modified).toBe(true);

    const editedNum = computeNextLocksOnTouch(['authors', 'title', 'work_type'], 'year', false, 2002);
    expect(editedNum.lockedFields).toEqual(['authors', 'title', 'work_type', 'year']);
    expect(editedNum.modified).toBe(true);
  });

  it('builds a valid save payload when clearing all wrong fetched fields including year', () => {
    const meta = makeJolliffeMeta();

    for (const emptyYear of ['', '   ', null, undefined] as const) {
      const res = buildManualMetadataEditPayload(meta, makeClearedJolliffeFields(emptyYear), false);
      expect(res.ok).toBe(true);
      if (!res.ok) continue;
      expect(res.payload.document_id).toBe(2002);
      expect(res.payload.work_type).toBe('book');
      expect(res.payload.title).toBe('Principal Component Analysis');
      expect(res.payload.authors).toEqual([
        { family_name: 'Jolliffe', given_name: 'I. T.', role: 'author' },
      ]);
      expect(res.payload.year).toBeNull();
      expect(res.payload.pages).toBeNull();
      expect(res.payload.journal_abbrev).toBeNull();
      expect(res.payload.journal_full).toBeNull();
      expect(res.payload.volume).toBeNull();
      expect(res.payload.issue_number).toBeNull();
      expect(res.payload.doi).toBeNull();
      expect(res.payload.mr_number).toBeNull();
      expect(res.payload.issn).toBeNull();
      expect(res.payload.url).toBeNull();
      expect(res.payload.msc_primary).toBeNull();
      expect(res.payload.msc_secondary).toEqual([]);
    }
  });

  it('builds a valid save payload when editing year to a new integer', () => {
    const meta = makeJolliffeMeta();
    const res = buildManualMetadataEditPayload(meta, makeClearedJolliffeFields('2002'), false);
    expect(res.ok).toBe(true);
    if (res.ok) {
      expect(res.payload.year).toBe(2002);
    }
  });
});
