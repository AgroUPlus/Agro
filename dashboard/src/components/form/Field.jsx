/**
 * One labelled form control, with its hint and its error.
 *
 * Every screen here had been writing the label, the input and the error message as three
 * unrelated elements, which is why focus rings, error colours and spacing drifted apart between
 * them. It also meant no input was ever associated with the text explaining it: a screen reader
 * announced "Username, edit text" and never the hint or the error sitting right below it.
 *
 * The control is a render prop rather than a child element so the wiring — `id`,
 * `aria-describedby`, `aria-invalid` — is computed here and cannot be forgotten at a call site.
 */
export default function Field({ id, label, hint, error, optional, children }) {
  const hintId = hint ? `${id}-hint` : null;
  const errorId = error ? `${id}-error` : null;
  const describedBy = [errorId, hintId].filter(Boolean).join(' ') || undefined;

  return (
    <div className="field">
      <label className="field-label" htmlFor={id}>
        {label}
        {optional && <span className="field-optional">optional</span>}
      </label>

      {children({
        id,
        'aria-describedby': describedBy,
        'aria-invalid': error ? true : undefined,
        invalid: Boolean(error)
      })}

      {/* The error replaces the hint rather than stacking on it: once a field is wrong, the
          instruction that led there is no longer the thing to read. */}
      {error ? (
        <p className="field-error" id={errorId} role="alert">{error}</p>
      ) : hint ? (
        <p className="field-hint" id={hintId}>{hint}</p>
      ) : null}
    </div>
  );
}
