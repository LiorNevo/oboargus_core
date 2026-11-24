# Bundled exchange-calendar seed

`us_equities_2016_2028.json` contains the initial calendars for the six
US-equity venue codes produced by the enabled identifier mapping:

- `XNAS` — Nasdaq Global Select Market
- `XNYS` — New York Stock Exchange
- `ARCX` — NYSE Arca
- `XASE` — NYSE American
- `BATS` — Cboe BZX Exchange
- `OTC` — OTC Markets (legacy 3-character pseudo-MIC)

## Source and verification

The closure and early-close dates are generated from the public US-equity
holiday rules (New Year's Day, Martin Luther King Jr. Day, Washington's
Birthday, Good Friday, Memorial Day, Juneteenth from 2022, Independence Day,
Labor Day, Thanksgiving, Christmas, the day after Thanksgiving, Christmas Eve,
and July 3 when it is a weekday). Known extraordinary closures are included:
2018-12-05 (national day of mourning for George H. W. Bush) and 2025-01-09
(national day of mourning for Jimmy Carter).

**Verification status:** verified 2026-11-24 against
[`pandas_market_calendars`](https://github.com/rsheftel/pandas_market_calendars)
5.6.1 (MIT licence). Its `NASDAQ`, `NYSE`, and `BATS` calendars agreed with all
3267/3265 sessions, opening times, and closing times in this dataset. The
verification found and removed two phantom New Year's Day closures
(2021-12-31 and 2027-12-31): the NYSE does not observe a Saturday New Year's
Day on the preceding Friday.

Exchange notices remain the authority if a future ad-hoc closure changes a
session after this verification date. Redistribution: the dataset contains
factual schedule dates, not library code; `pandas_market_calendars` is used
only as a verification tool and is not redistributed.

## Seed behavior

The server seeds a calendar only when its venue code is absent. It never
overwrites an operator-managed calendar, and it creates no demo market-price
observations. Re-running the seed is a no-op after the first successful insert.
