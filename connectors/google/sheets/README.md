# Google Sheets

The initial slice reads spreadsheet/sheet properties and one explicitly named
A1 rectangle. It uses native bearer credential custody and native HTTP; no
spreadsheet client, loop, OAuth store or hosted driver is added.

The selected Google authorization must include
`https://www.googleapis.com/auth/spreadsheets.readonly` (or a deliberately
broader existing scope). Bind its access token through the local credential
boundary. Token acquisition/refresh stays with the existing authorization owner;
this manifest neither reconnects the Google app nor claims hosted Connect is
wired. Spreadsheet access still depends on the selected Google principal's
permissions. Drive file discovery is a separate provider job.

`spreadsheet.read` excludes cell data. `values.read` accepts a sheet-qualified
rectangle in columns A-Z and rows 1-1000, at most 26,000 cells. Query encoding
belongs to the native HTTP driver, including quoted sheet names; the range is
sent as `ranges` to `values:batchGet`, not interpolated into a URL path.

Choose `FORMULA`, `UNFORMATTED_VALUE` or `FORMATTED_VALUE` explicitly. The
provider omits trailing empty rows/cells; do not treat array dimensions as the
sheet's declared dimensions or equate an empty cell with a missing record.
Unformatted dates use serial numbers; formatted values follow spreadsheet
locale. Formula text is untrusted content. This read slice changes no values,
formulas, formats or permissions. Writes need the existing native effect and
readback contract and will be added at that boundary.

Source contracts: [spreadsheet metadata](https://developers.google.com/workspace/sheets/api/reference/rest/v4/spreadsheets/get),
[bounded value reads](https://developers.google.com/workspace/sheets/api/reference/rest/v4/spreadsheets.values/batchGet),
[authorization](https://developers.google.com/workspace/sheets/api/scopes).
No live account access has been verified for this source addition.
