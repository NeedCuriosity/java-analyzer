import java.util.List;

public class ControlFlow {
    public int loops(List<Integer> values) {
        int total = 0;
        outer:
        for (int value : values) {
            for (int i = 0; i < value; i++) {
                if (i > 5) {
                    continue outer;
                }
                if (i == value - 1) {
                    break outer;
                }
                total += i;
            }
        }
        return total;
    }

    public int doWhile(int n) {
        int steps = 0;
        do {
            n = n / 2;
            steps++;
        } while (n > 0);
        return steps;
    }

    public int assignedLater(boolean flag, int n) {
        int result;
        if (flag) {
            result = n;
        } else if (n > 3) {
            result = 3;
        } else {
            return 0;
        }
        return result;
    }

    public String ternary(int n) {
        String sign = n > 0 ? "pos" : n < 0 ? "neg" : "zero";
        int magnitude = n >= 0 ? n : -n;
        return sign + magnitude;
    }

    public int switchOld(int n) {
        int result;
        switch (n) {
            case 1:
                result = 10;
                break;
            case 2:
            case 3:
                result = 20;
                break;
            default:
                result = 0;
        }
        return result;
    }

    public int whileTrue(int n) {
        while (true) {
            if (n > 100) {
                return n;
            }
            n *= 2;
        }
    }

    public long compound(int n) {
        long acc = 1;
        acc += n;
        acc <<= 2;
        acc ^= n;
        int small = (int) acc;
        small++;
        return acc + small;
    }

    public boolean logic(boolean a, boolean b, Object o) {
        boolean both = a && b;
        boolean either = a | b;
        boolean isText = o instanceof String;
        return both ^ either || isText;
    }
}
