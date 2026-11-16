//? module alpha
//? module beta
//- /alpha/src/main/java/com/acme/Config.java module:alpha
package com.acme;

public class Config {
    public int alphaPort = 8080;
}
//- /beta/src/main/java/com/acme/Config.java module:beta
package com.acme;

public class Config {
    public String betaHost = "localhost";
}
//- /alpha/src/main/java/com/acme/Application.java module:alpha
package com.acme;

class Application {
    void run(Config config) {
        config./*@alpha*/;
    }
}
//- /beta/src/main/java/com/acme/Application.java module:beta
package com.acme;

class Application {
    void run(Config config) {
        config./*@beta*/;
    }
}
//? complete alpha has alphaPort
//? complete alpha lacks betaHost
//? complete beta has betaHost
//? complete beta lacks alphaPort
