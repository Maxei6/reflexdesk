# Plan 15 — Product localization

**Priority:** P2  
**Status:** NOT STARTED

## Objective

Speech is multilingual already; the product UI and recovery messages should be
too.

## Work packages

1. Extract all visible strings into locale catalogs.
2. Locale fallback rules.
3. OS-locale default on first run.
4. User-selectable UI language separate from speech language.
5. RTL layout test for Arabic/Hebrew.
6. plural/date/number formatting.
7. localize installer/update/onboarding errors.
8. translation QA for safety/confirmation wording.

## Initial languages

English and Italian first, then Spanish, French, German and Portuguese based on
contributors/user demand.

## Definition of done

No production UI string is hard-coded in the main application flow, and safety
confirmations retain equivalent meaning across supported locales.
