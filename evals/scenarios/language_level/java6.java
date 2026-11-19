//- /app/Legacy.java
package app;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileReader;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

interface Greeter {
    String greet(String name);

    /*@def*/default String hi() { return greet("hi"); }

    /*@stat*/static Greeter create() { return null; }
}

class Legacy {
    int mask = /*@bin*/0b1010;
    long big = /*@under*/1_000_000L;
    List<String> names = new ArrayList/*@diamond*/<>();
    Map<String, List<Integer>> map = new HashMap<String, List<Integer>>();

    String read(File file) throws IOException {
        try /*@twr*/(BufferedReader in = new BufferedReader(new FileReader(file))) {
            return in.readLine();
        }
    }

    void parse(String s) {
        try {
            Integer.parseInt(s);
            Class.forName(s);
        } catch (NumberFormatException /*@multi*/| ClassNotFoundException e) {
            throw new RuntimeException(e);
        }
    }

    int code(String s) {
        switch (s) {
            case /*@str*/"a":
                return 1;
            case "b":
                return 2;
            default:
                return 0;
        }
    }

    void sort(List<String> list) {
        Collections.sort(list, (a, b) /*@lambda*/-> a.compareTo(b));
        Runnable r = this/*@ref*/::toString;
        r.run();
    }

    int legacy(List<String> list) {
        Collections.sort(list, new Comparator<String>() {
            public int compare(String a, String b) {
                return a.length() - b.length();
            }
        });
        switch (mask) {
            case 1:
                return 0x1F;
            default:
                return 1000;
        }
    }
}
//? level 6
//? diag feature-not-available def
//? diag feature-not-available stat
//? diag feature-not-available bin
//? diag feature-not-available under
//? diag feature-not-available diamond
//? diag feature-not-available twr
//? diag feature-not-available multi
//? diag feature-not-available str
//? diag feature-not-available lambda
//? diag feature-not-available ref
//? diag exact
//? diag only feature-not-available
