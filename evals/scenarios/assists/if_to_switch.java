class Main {
    enum Color { RED, GREEN, BLUE }

    String ints(int n) {
        String s;
        /*@ints*/if (n == 1) {
            s = "one";
        } else if (n == 2 || n == 3) {
            s = "few";
            System.out.println(s);
        } else {
            s = "many";
        }
        return s;
    }

    void strings(String cmd) {
        /*@strings*/if (cmd.equals("start")) {
            System.out.println("go");
        } else if (cmd.equals("stop")) {
            throw new IllegalStateException();
        }
    }

    void enums(Color c) {
        /*@enums*/if (c == Color.RED) {
            System.out.println(1);
        } else if (Color.GREEN == c) {
            System.out.println(2);
        }
    }

    void mixed(int n, int m) {
        /*@mixed*/if (n == 1) {
            System.out.println(1);
        } else if (m == 2) {
            System.out.println(2);
        }
    }

    void nullSafe(String cmd) {
        /*@null_safe*/if ("a".equals(cmd)) {
            System.out.println(1);
        } else if ("b".equals(cmd)) {
            System.out.println(2);
        }
    }

    void loop(int[] xs) {
        for (int x : xs) {
            /*@breaks*/if (x == 1) {
                break;
            } else if (x == 2) {
                System.out.println(x);
            }
        }
    }
}
//? assist replace_if_with_switch ints ; has "        switch (n) {\n            case 1 -> s = \"one\";\n            case 2, 3 -> {\n                s = \"few\";\n                System.out.println(s);\n            }\n            default -> s = \"many\";\n        }"
//? assist replace_if_with_switch strings ; has "switch (cmd) {\n            case \"start\" -> System.out.println(\"go\");\n            case \"stop\" -> throw new IllegalStateException();\n        }"
//? assist replace_if_with_switch enums ; has "case RED -> System.out.println(1);\n            case GREEN -> System.out.println(2);"
//? noassist replace_if_with_switch mixed
//? noassist replace_if_with_switch null_safe
//? noassist replace_if_with_switch breaks
