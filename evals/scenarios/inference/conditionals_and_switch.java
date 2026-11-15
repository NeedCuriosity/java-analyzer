import java.util.ArrayList;
import java.util.LinkedList;
import java.util.List;

class Main {
    enum Kind { A, B }

    void run(boolean flag, Kind kind, Object o, int n) {
        var /*@mixed*/mixed = flag ? 1 : 2.0;
        var /*@same*/same = flag ? "a" : "b";
        var /*@boxed*/boxed = flag ? Integer.valueOf(1) : 2;
        var /*@sw*/sw = switch (kind) {
            case A -> "a";
            case B -> "b";
        };
        var /*@num*/num = switch (n) {
            case 1 -> 1;
            default -> 2L;
        };
        var /*@cast*/cast = (CharSequence) o;
        var /*@arr*/arr = new int[][] { { 1 } };
        var /*@row*/row = arr[0];
        var /*@cell*/cell = arr[0][0];
        var /*@incr*/incr = n++;
        var /*@shift*/shift = n << 2L;
        var /*@cmp*/cmp = n < 3;
        var /*@neg*/neg = -n;
        byte small = 1;
        var /*@promoted*/promoted = small + small;
        var /*@concat*/concat = n + "" + flag;
        if (o instanceof String /*@s*/s) {
            var /*@trimmed*/trimmed = s.trim();
        }
    }
}
//? hover mixed ~ "double mixed"
//? hover same ~ "String same"
//? hover boxed ~ "int boxed"
//? hover sw ~ "String sw"
//? hover num ~ "long num"
//? hover cast ~ "CharSequence cast"
//? hover arr ~ "int[][] arr"
//? hover row ~ "int[] row"
//? hover cell ~ "int cell"
//? hover incr ~ "int incr"
//? hover shift ~ "int shift"
//? hover cmp ~ "boolean cmp"
//? hover neg ~ "int neg"
//? hover promoted ~ "int promoted"
//? hover concat ~ "String concat"
//? hover s ~ "String s"
//? hover trimmed ~ "String trimmed"
//? diag none
