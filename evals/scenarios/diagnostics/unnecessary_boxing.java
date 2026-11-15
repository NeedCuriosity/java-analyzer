import java.util.List;

class Main {
    private Integer cached;

    Integer box(int x) {
        Integer a = /*@b1*/Integer.valueOf(x);
        cached = /*@b2*/Integer.valueOf(x + 1);
        return /*@b3*/Integer.valueOf(a + cached);
    }

    @SuppressWarnings("deprecation")
    Double boxNew(double d) {
        return /*@b4*/new Double(d);
    }

    int unbox(Integer n, Long l, Boolean flag) {
        int a = n/*@u1*/.intValue();
        long b = l/*@u2*/.longValue() * 2;
        if (flag/*@u3*/.booleanValue() && a > 0) {
            a += n/*@u4*/.intValue();
        }
        return a + (int) b;
    }

    void needed(Integer n, Integer m, List<Integer> list, String s, long big) {
        Long widened = Long.valueOf(3);
        Integer parsed = Integer.valueOf(s);
        Object o = Integer.valueOf(1);
        var boxed = Integer.valueOf(4);
        var unboxed = n.intValue();
        list.add(Integer.valueOf(2));
        boolean same = n.intValue() == m.intValue();
        boolean mixed = n/*@u5*/.intValue() == big;
        String text = "v" + n.intValue();
        long wide = n.intValue();
        double ratio = n.doubleValue();
        int hash = Integer.valueOf(3).hashCode();
        System.out.println("" + boxed + unboxed + mixed + widened + parsed + o + same + text + wide + ratio + hash + big);
    }
}
//? diag unnecessary-boxing b1
//? diag unnecessary-boxing b2
//? diag unnecessary-boxing b3
//? diag unnecessary-boxing b4
//? diag unnecessary-boxing u1
//? diag unnecessary-boxing u2
//? diag unnecessary-boxing u3
//? diag unnecessary-boxing u4
//? diag unnecessary-boxing u5
//? diag only unnecessary-boxing
//? assist remove_unnecessary_boxing b1 ; has "Integer a = x;"
//? assist remove_unnecessary_boxing b3 ; has "return a + cached;"
//? assist remove_unnecessary_boxing u1 ; has "int a = n;"
//? assist remove_unnecessary_boxing u3 ; has "if (flag && a > 0)"
