SELECT
    trim(station) AS station,
    lower(trim(habitat)) AS habitat,
    individuals
FROM data
WHERE individuals IS NOT NULL
ORDER BY station, individuals;
