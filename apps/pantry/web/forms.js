/*
 * Project:  Privatium™  |  File: apps/pantry/web/forms.js
 * Authors:  Gabriel Mongefranco (@gabrielmongefranco)
 * Created:  2026-09-07  |  Modified: 2026-09-07
 * Summary:  Reading the forms and saying what is wrong with them, in place. Two fences
 *           stand between a typed value and the log, and this file is the first: it checks
 *           what it can and puts each message beneath the field it belongs to, with
 *           aria-invalid on the field and the focus moved to the first bad one. The second
 *           fence is the node's, which validates every declared column, NOT NULL and each
 *           CHECK from schema.sql before it appends (spec/data-api.md §2) and names the
 *           column it refused; showRefusal puts that message under the same field. A
 *           quantity is read as text and stays text: the value the person typed is what
 *           reaches the DECIMAL column, at the scale the column declares.
 *           See main README.md for full license information.
 */

/** A quantity: digits, optionally a point and up to the three decimal places the column
 * declares. Deliberately not a number — `Number('0.1')` is already not 0.1. */
const AMOUNT = /^\d{1,15}(\.\d{1,3})?$/;

/** Which input each declared column is typed into, for a refusal that names one. */
const FIELDS = {
  name: 'b-name',
  icon: 'b-name',
  unit: 'b-unit',
  shelf_id: 'b-shelf',
  stored_on: 'b-stored',
  expires_on: 'b-expires',
  amount: 'b-amount',
};

const $ = id => document.getElementById(id);

/**
 * Show a message beneath one field and mark the field invalid.
 * @param {string} inputId The field's id; its message lives at `<inputId>-err`.
 * @param {string} message What is wrong, in the person's own terms.
 */
export function setFieldError(inputId, message) {
  const field = $(inputId);
  const box = $(`${inputId}-err`);
  if (!field || !box) return;
  field.setAttribute('aria-invalid', 'true');
  box.textContent = message;
  box.removeAttribute('hidden');
}

/**
 * Show a message about the form as a whole — a refusal that names no column.
 * @param {string} formId The form's id; its message lives at `<formId>-err`.
 * @param {string} message What the node said.
 */
export function setFormError(formId, message) {
  const box = $(`${formId}-err`);
  if (!box) return;
  box.textContent = message;
  box.removeAttribute('hidden');
}

/**
 * Clear every message in one form.
 * @param {string} formId The form's id.
 */
export function clearErrors(formId) {
  const form = $(formId);
  if (!form) return;
  for (const box of form.querySelectorAll('.err')) {
    box.textContent = '';
    box.setAttribute('hidden', 'hidden');
  }
  for (const field of form.querySelectorAll('[aria-invalid]')) {
    field.removeAttribute('aria-invalid');
  }
}

/**
 * Show the problems, focus the first bad field, and say whether there were any.
 * @param {string} formId The form's id.
 * @param {Array<[string, string]>} problems Field id and message, in the form's own order.
 * @returns {boolean} True when the form is clean.
 */
function report(formId, problems) {
  clearErrors(formId);
  for (const [inputId, message] of problems) setFieldError(inputId, message);
  if (problems.length > 0) $(problems[0][0])?.focus();
  return problems.length === 0;
}

/**
 * What is wrong with a typed quantity, if anything.
 * @param {string} amount The field's value, trimmed.
 * @returns {string|null} A sentence, or null when the value is usable.
 */
export function amountProblem(amount) {
  if (amount === '') return 'Say how much.';
  if (!AMOUNT.test(amount)) {
    return 'Write a number such as 2 or 1.5, with at most three decimal places, and no minus sign.';
  }
  if (!/[1-9]/.test(amount)) return 'Zero would record nothing. Write how much.';
  return null;
}

/**
 * Read the new-shelf form.
 * @returns {{name: string}|null} The shelf, or null when the form said what is wrong.
 */
export function readShelf() {
  const name = $('s-name').value.trim();
  const problems = name === '' ? [['s-name', 'Give the shelf a name, such as "Freezer drawer 1".']] : [];
  return report('shelf-form', problems) ? { name } : null;
}

/**
 * Read the add-batch form.
 * @returns {{batch: object, amount: string}|null} The batch row and its first stock, or
 *   null when the form said what is wrong.
 */
export function readBatch() {
  const name = $('b-name').value.trim();
  const amount = $('b-amount').value.trim();
  const shelfId = $('b-shelf').value;
  const storedOn = $('b-stored').value;
  const expiresOn = $('b-expires').value;
  const picked = document.querySelector('input[name="icon"]:checked');

  const problems = [];
  if (name === '') problems.push(['b-name', 'Say what it is, such as "Chicken soup".']);
  const wrong = amountProblem(amount);
  if (wrong) problems.push(['b-amount', wrong]);
  if (!shelfId) problems.push(['b-shelf', 'Add a shelf first; a batch has to go somewhere.']);
  if (!storedOn) problems.push(['b-stored', 'Say the day it went in.']);
  if (expiresOn && storedOn && expiresOn < storedOn) {
    problems.push(['b-expires', 'The use-by date is before the day it went in. Leave it blank if the package says nothing.']);
  }
  if (!report('batch-form', problems)) return null;

  return {
    batch: {
      name,
      icon: picked ? picked.value : 'box-seam',
      unit: $('b-unit').value,
      shelf_id: shelfId,
      stored_on: storedOn,
      expires_on: expiresOn === '' ? null : expiresOn,
    },
    amount,
  };
}

/**
 * Put a node's refusal beneath the field it names, or on the form when it names none.
 * @param {string} formId The form that was submitted.
 * @param {Error} error The error `pv.append` threw; `detail.column` names a column.
 * @returns {string} The sentence shown, for the status line.
 */
export function showRefusal(formId, error) {
  const column = error.detail && error.detail.column;
  const message = error.message || 'The node refused the change.';
  const inputId = column && FIELDS[column];
  clearErrors(formId);
  if (inputId) setFieldError(inputId, message);
  else setFormError(formId, message);
  return message;
}

/**
 * Reset the add-batch form to its next-entry state, keeping the shelf and the unit.
 */
export function resetBatch() {
  clearErrors('batch-form');
  $('b-name').value = '';
  $('b-amount').value = '';
  $('b-expires').value = '';
  $('b-name').focus();
}
