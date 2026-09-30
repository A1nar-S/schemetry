import type { SQLNamespace } from '@codemirror/lang-sql';
import type { CompletionMetadata } from './types';

// CodeMirror completion types, which pick the icon shown next to each option.
function objectType(objectType: string): string {
  switch (objectType) {
    case 'SEQUENCE': return 'variable';
    case 'PACKAGE': return 'namespace';
    case 'TYPE': return 'class';
    default: return 'function';
  }
}

/**
 * Builds the SQL editor's completion schema: tables/views (with their columns) and
 * other objects, nested under the schema name so both `EMPLOYEES` and
 * `HR.EMPLOYEES` complete.
 */
export function toCompletionSchema(meta: CompletionMetadata): SQLNamespace {
  const tables: Record<string, SQLNamespace> = {};
  for (const r of meta.relations) {
    tables[r.name] = {
      self: { label: r.name, type: 'type', detail: r.kind.toLowerCase() },
      children: r.columns.map(c => ({ label: c.name, type: 'property', detail: c.data_type.toLowerCase() })),
    };
  }
  for (const o of meta.objects) {
    if (!(o.name in tables)) {
      tables[o.name] = {
        self: { label: o.name, type: objectType(o.object_type), detail: o.object_type.toLowerCase() },
        children: [],
      };
    }
  }
  return { [meta.schema]: tables };
}
