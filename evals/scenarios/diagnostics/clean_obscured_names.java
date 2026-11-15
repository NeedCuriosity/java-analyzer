//- /app/BucketPolicy.java
package app;

import java.util.List;

public class BucketPolicy {
    private String Version;
    private List<Statement> Statement;

    public static class Statement {
        private String Effect;
        public String Resource;

        public static Statement of(String effect) {
            Statement statement = new Statement();
            statement.Effect = effect;
            return statement;
        }

        public String effect() { return Effect; }
    }

    public static class Version {
        public static final String CURRENT = "2012-10-17";
    }

    public List<Statement> statements() { return Statement; }
}
//- /app/Main.java
package app;

class Main {
    String run() {
        BucketPolicy.Statement statement = BucketPolicy.Statement./*@call*/of("Allow");
        String version = BucketPolicy.Version./*@constant*/CURRENT;
        return statement.effect() + version + BucketPolicy.Statement.of("Deny").Resource;
    }
}
//? diag none
//? hover call ~ "Statement of(String effect)"
//? hover constant ~ "String CURRENT"
