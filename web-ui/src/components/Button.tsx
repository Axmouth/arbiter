import type { ButtonHTMLAttributes } from 'react'

export type ButtonVariant =
  | 'primary'
  | 'secondary'
  | 'danger'
  | 'positive'
  | 'warning'
  | 'ghost'

/**
 * The single button primitive. Styling lives in the `.btn`/`.btn-*` classes in index.css;
 * this just selects a variant and forwards native button props. Router links that act as
 * buttons use the same classes directly (`className="btn btn-primary"`). Defaults to
 * `type="button"` so a button inside a form does not submit unless it asks to.
 */
export function Button({
  variant = 'secondary',
  className = '',
  type = 'button',
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant }) {
  return <button type={type} className={`btn btn-${variant} ${className}`.trim()} {...props} />
}
