export const DDL_FILE_ENCODINGS: { value: string; label: string }[] = [
  { value: 'utf8', label: 'UTF-8' },
  { value: 'utf8-bom', label: 'UTF-8 with BOM' },
  { value: 'windows-1257', label: 'Windows-1257 (Baltic / EE8MSWIN1257)' },
];

// Object types the DDL view groups by, and the default file extension used for each
// when saving to the schema folder. Must mirror DEFAULT_DDL_EXTENSIONS in src-tauri/src/services/settings.rs.
export const DDL_OBJECT_TYPES: string[] = [
  'TABLE', 'VIEW', 'MATERIALIZED VIEW', 'PROCEDURE', 'FUNCTION', 'PACKAGE',
  'PACKAGE BODY', 'TRIGGER', 'SEQUENCE', 'SYNONYM', 'TYPE', 'JOB',
];

// What gets written to the schema folder when saving an object's DDL: the raw "code"
// file, the idempotent migration script, or both (the historical default).
export const STORAGE_MODES: { value: string; label: string }[] = [
  { value: 'both', label: 'Both' },
  { value: 'code', label: 'Code only' },
  { value: 'migration', label: 'Migration only' },
];

// How saved-to-folder filenames are named. "flyway" names the migration file
// `V<n>__schema_description.ext` and the code file `R__name.ext`, marking it as a
// Flyway repeatable migration.
export const NAMING_CONVENTIONS: { value: string; label: string }[] = [
  { value: 'timestamp', label: 'Timestamp' },
  { value: 'flyway', label: 'Flyway (V__ / R__)' },
];

// Where migration files are filed: under a folder named for the current calendar year,
// or a manually-set version label.
export const MIGRATION_FOLDER_MODES: { value: string; label: string }[] = [
  { value: 'year', label: 'Year' },
  { value: 'version', label: 'Version label' },
];
