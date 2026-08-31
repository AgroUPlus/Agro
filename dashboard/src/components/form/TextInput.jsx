/**
 * The dashboard's text input.
 *
 * `invalid` comes from [Field] rather than being passed by hand, so an input cannot show a red
 * border while the error text is missing, or the reverse.
 */
export default function TextInput({ invalid, className = '', ...props }) {
  return (
    <input
      {...props}
      className={`field-input${invalid ? ' field-input-invalid' : ''}${className ? ` ${className}` : ''}`}
    />
  );
}
