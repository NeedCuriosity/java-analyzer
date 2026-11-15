import java.util.List;

class Main {
    private Object field;
    private String s;

    int simple(Object o) {
        if (o instanceof String) {
            /*@p1*/String text = (String) o;
            return text.length();
        }
        return 0;
    }

    int chained(Object o, boolean ready) {
        if (ready && o instanceof List<?> && !ready) {
            /*@p2*/List<?> list = (List<?>) o;
            return list.size();
        }
        return -1;
    }

    int notFirst(Object o) {
        if (o instanceof String) {
            System.out.println("x");
            String text = (String) o;
            return text.length();
        }
        return 0;
    }

    int fieldTested() {
        if (field instanceof String) {
            String text = (String) field;
            return text.length();
        }
        return 0;
    }

    int nameUsedLater(Object o) {
        if (o instanceof String && s.isEmpty()) {
            String s = (String) o;
            return s.length();
        }
        return 0;
    }

    int otherType(Object o) {
        if (o instanceof CharSequence) {
            String text = (String) o;
            return text.length();
        }
        return 0;
    }

    int alreadyPattern(Object o) {
        if (o instanceof String text) {
            return text.length();
        }
        return 0;
    }
}
//? diag pattern-variable p1
//? diag pattern-variable p2
//? diag only pattern-variable
//? assist fix_redundancy p1 ; has "if (o instanceof String text) {\n            return text.length();"
//? assist fix_redundancy p2 ; has "if (ready && o instanceof List<?> list && !ready) {\n            return list.size();"
