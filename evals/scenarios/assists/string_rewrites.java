class Main {
    String describe(String name, int count, double ratio) {
        return /*@fmt*/"Name: " + name + ", count " + count + " (" + ratio + "%)";
    }

    String sum(int a, int b) {
        return /*@sum*/a + b + " total";
    }

    String noLiteral(String a, String b) {
        return /*@plain*/a + b;
    }

    String query() {
        return /*@sql*/"SELECT id\n" +
            "FROM users\n" +
            "WHERE name = \"x\" \n";
    }

    String noTrailing() {
        return /*@tail*/"first\nsecond";
    }

    String oneLine() {
        return /*@one*/"just one line\n";
    }

    String mixed(String table) {
        return /*@mixed*/"SELECT *\n" + "FROM " + table;
    }
}
//? assist concat_to_format fmt ; has "return String.format(\"Name: %s, count %d (%s%%)\", name, count, ratio);"
//? assist concat_to_format sum ; has "String.format(\"%d total\", a + b)"
//? noassist concat_to_format plain
//? assist concat_to_text_block sql ; has "return \"\"\"\n            SELECT id\n            FROM users\n            WHERE name = \"x\"\\s\n            \"\"\";"
//? assist concat_to_text_block tail ; has "return \"\"\"\n            first\n            second\"\"\";"
//? noassist concat_to_text_block one
//? noassist concat_to_text_block mixed
//? assist concat_to_format mixed ; has "String.format(\"SELECT *\\nFROM %s\", table)"
