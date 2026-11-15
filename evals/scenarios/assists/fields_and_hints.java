//- /app/Config.java
package app;

public class Config {
    public static final int /*@const*/TIMEOUT = 30 * 1000;
    static final String PREFIX = "app.";
}
//- /app/Main.java
package app;

import java.util.List;

class Main {
    private int base = 1;

    int wait(int extra) {
        return Config./*@use*/TIMEOUT + extra;
    }

    String key(String name) {
        return /*@f1*/Config.PREFIX + "key"/*@f2*/ + name;
    }

    int sum(int a, int b) {
        int total = /*@g1*/a * base/*@g2*/;
        return total;
    }

    List<String> chain(List<String> names) {
        return names.stream()
            .map(String::trim)/*@h1*/
            .filter(s -> !s.isEmpty())
            .map(String::length)/*@h2*/
            .map(String::valueOf)
            .toList();
    }

    //region helpers
    void a() {}
    void b() {}
    //endregion
}
//? assist inline_constant use ; has "return 30 * 1000 + extra;" ; lacks "TIMEOUT"
//? assist inline_constant const ; has "return 30 * 1000 + extra;"
//? assist extract_field f1 f2 ; has "private final String extracted = Config.PREFIX + \"key\";" ; has "return extracted + name;"
//? assist extract_field g1 g2 ; has "private int extracted;" ; has "this.extracted = a * base;\n        int total = extracted;"
//? hint h1 "Stream<String>"
//? hint h2 "Stream<Integer>"
