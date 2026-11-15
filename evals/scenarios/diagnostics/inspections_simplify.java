import java.util.Date;
import java.util.List;

class Main {
    boolean flag(boolean b, List<String> names, String text, Object o) {
        if (/*@pb*/b == true) {
            System.out.println("yes");
        }
        boolean empty = /*@size*/names.size() == 0;
        boolean some = /*@size2*/0 < text.length();
        String s = /*@cast*/(String) text;
        int n = ((/*@cast2*/String) text).length();
        Object kept = (String) o;
        Date d = new /*@dep*/Date(2020, 1, 1);
        int year = d./*@dep2*/getYear();
        return empty || some || n > 0 || s.isEmpty() || kept == null || year > 0;
    }

    /*@rif*/boolean positive(int x) {
        /*@rif2*/if (x > 0) {
            return true;
        } else {
            return false;
        }
    }

    boolean negative(int x) {
        /*@rif3*/if (x < 0) return false;
        return true;
    }

    @Deprecated
    int old() {
        return new Date().getYear();
    }
}
//? diag pointless-boolean pb
//? diag use-is-empty size
//? diag use-is-empty size2
//? diag redundant-cast cast
//? diag redundant-cast cast2
//? diag deprecated dep
//? diag deprecated dep2
//? diag redundant-if rif2
//? diag redundant-if rif3
//? diag only redundant-cast
//? diag only deprecated
//? assist simplify pb ; has "if (b) {"
//? assist simplify size ; has "boolean empty = names.isEmpty();"
//? assist simplify size2 ; has "boolean some = !text.isEmpty();"
//? assist simplify cast ; has "String s = text;"
//? assist simplify cast2 ; has "int n = text.length();"
//? assist simplify rif2 ; has "        return x > 0;\n    }"
//? assist simplify rif3 ; has "        return x >= 0;\n    }"
