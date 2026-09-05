# Official Flyway images don't bundle the Oracle JDBC driver (Oracle's license
# doesn't allow redistribution), so pull it from Maven Central at build time
# and drop it into Flyway's driver auto-load directory.
FROM flyway/flyway:13.5.0-alpine

ARG OJDBC_VERSION=23.5.0.24.07

RUN wget -q -O /flyway/drivers/ojdbc11.jar \
    "https://repo1.maven.org/maven2/com/oracle/database/jdbc/ojdbc11/${OJDBC_VERSION}/ojdbc11-${OJDBC_VERSION}.jar"
