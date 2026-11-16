//- /app/Main.java
package app;

import java.util.List;
import util.Helper;

class Main {
    void run(Repo repo, List<String> names) {
        repo./*@f1*/items = names;
        int max = Repo./*@f2*/MAX_SIZE;
        String tag = Helper./*@f3*/prefix;
        int n = Limits./*@f4*/DEFAULT;
        System.out.println(max + tag + n);
        System.out.println(repo./*@f5*/size);
    }
}

//- /app/Repo.java
package app;

class Repo {
}

//- /app/Limits.java
package app;

interface Limits {
}

//- /util/Helper.java
package util;

public class Helper {
}

//? assist create_field_in_class f1 ; has "class Repo {\n\n    List<String> items;\n}" ; has "import java.util.List;"
//? assist create_field_in_class f2 ; has "    static final int MAX_SIZE = 0;"
//? assist create_field_in_class f3 ; has "    public static String prefix;"
//? assist create_field_in_class f4 ; has "interface Limits {\n\n    int DEFAULT = 0;\n}"
//? assist create_field_in_class f5 ; has "    Object size;"
