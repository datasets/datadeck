# DataHub.io datasets

DataDeck can open a public DataHub resource from its download URL today. On 25 November 2026, the native loader read 67 records from the annual Mauna Loa CO₂ CSV. Its first record was 1959, 315.98 ppm, with an uncertainty of 0.12 ppm. [Dataset](https://datahub.io/core/co2-ppm) · [Download the annual CSV](https://datahub.io/core/co2-ppm/_r/-/data/co2-annmean-mlo.csv)

## Pull a resource from a dataset page

The CO₂ dataset links a Frictionless data package descriptor under `_r/-/datapackage.json`. A public HTTPS request returned six CSV resources, each with a name and relative path, plus the package license. For example, the annual Mauna Loa resource path `data/co2-annmean-mlo.csv` resolves relative to the descriptor URL. [Public descriptor](https://datahub.io/core/co2-ppm/_r/-/datapackage.json)

The proposed desktop flow accepts a dataset page or descriptor URL, resolves its descriptor, then lets the reader choose a supported resource. Keep the page URL, selected resource path and license with the opened source so someone replaying a recipe can identify its input. Existing direct downloads can use the file loader immediately. A dataset without a descriptor can still open through a direct resource link.

## Publish a derived dataset

A proposed publish action stages the full query output in a fresh directory with a CSV, a root `README.md`, and `datapackage.json` naming the resource. Show the exact staged files, target publication and dataset name before sending them. A saved recipe can be included when the publisher chooses to share it. [DataPressr publishing guide](https://datapressr.datahub.io/docs/cli)

The installed DataHub.io CLI reported `dh version 0.1.0`. Its command help includes `dh login` and `dh publish`. Package reads use the public HTTPS interface. The CLI advertises these two publication modes and accepts an explicit dataset name via `--name`.

```sh
dh publish ./export --publication PUBLICATION --name DATASET
dh publish --github OWNER/REPO --branch main --subdir datasets/DATASET --publication PUBLICATION
```

DataPressr documents that publishing a local directory adds or overwrites files, previously published files can persist, and processing continues after upload. A desktop integration should distinguish upload acceptance from the public dataset becoming available. The CLI checks here used version and help output. Authentication, write behavior and processing still need an authorized end-to-end test. [Publishing and update behavior](https://datapressr.datahub.io/docs/cli)
