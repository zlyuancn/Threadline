package cn.zly.threadline

import android.app.Activity
import android.os.Bundle
import android.text.InputType
import android.util.Log
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import uniffi.tl_mobile.Forward
import uniffi.tl_mobile.ForwardRequest
import uniffi.tl_mobile.MobileException
import uniffi.tl_mobile.buildInfo
import uniffi.tl_mobile.startForward
import java.util.concurrent.Executors

/**
 * A0 spike：只验证「UniFFI → Rust 数据面 → 本地端口可用」这一条链路。
 * 不是产品界面（Compose / 前台服务在 A1 起）。凭据只在内存里，不落盘、不写日志。
 */
class SpikeActivity : Activity() {

    private val io = Executors.newSingleThreadExecutor()

    @Volatile
    private var forward: Forward? = null

    private lateinit var serverHost: EditText
    private lateinit var serverPort: EditText
    private lateinit var username: EditText
    private lateinit var password: EditText
    private lateinit var fingerprint: EditText
    private lateinit var listenHost: EditText
    private lateinit var listenPort: EditText
    private lateinit var remoteHost: EditText
    private lateinit var remotePort: EditText
    private lateinit var output: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(32, 32, 32, 32)
        }
        serverHost = field(root, "SSH 主机（公网端口）", "")
        serverPort = field(root, "SSH 端口", "22", InputType.TYPE_CLASS_NUMBER)
        username = field(root, "用户名", "")
        password = field(root, "密码", "", InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD)
        fingerprint = field(root, "已信任指纹（首次留空，会回显待确认指纹）", "")
        listenHost = field(root, "本地监听地址", "127.0.0.1")
        listenPort = field(root, "本地监听端口", "8123", InputType.TYPE_CLASS_NUMBER)
        remoteHost = field(root, "远端目标地址", "192.168.1.1")
        remotePort = field(root, "远端目标端口", "8123", InputType.TYPE_CLASS_NUMBER)

        root.addView(Button(this).apply {
            text = "启动转发"
            setOnClickListener { start() }
        })
        root.addView(Button(this).apply {
            text = "刷新状态"
            setOnClickListener { refresh() }
        })
        root.addView(Button(this).apply {
            text = "停止"
            setOnClickListener { stop() }
        })

        output = TextView(this).apply {
            text = buildInfo() + "\n"
            setTextIsSelectable(true)
        }
        root.addView(output)

        setContentView(ScrollView(this).apply { addView(root) })
        Log.i(TAG, buildInfo())
    }

    private fun field(parent: LinearLayout, label: String, value: String, inputType: Int = InputType.TYPE_CLASS_TEXT): EditText {
        parent.addView(TextView(this).apply { text = label })
        val edit = EditText(this).apply {
            setText(value)
            this.inputType = inputType
            isSingleLine = true
        }
        parent.addView(edit)
        return edit
    }

    private fun start() {
        val req = try {
            ForwardRequest(
                serverHost = serverHost.text.toString().trim(),
                serverPort = serverPort.text.toString().trim().toUShort(),
                username = username.text.toString(),
                password = password.text.toString(),
                trustedFingerprint = fingerprint.text.toString().trim().ifBlank { null },
                listenHost = listenHost.text.toString().trim(),
                listenPort = listenPort.text.toString().trim().toUShort(),
                remoteHost = remoteHost.text.toString().trim(),
                remotePort = remotePort.text.toString().trim().toUShort(),
                keepaliveSecs = 30u,
            )
        } catch (e: NumberFormatException) {
            log("端口必须是数字：${e.message}")
            return
        }

        log("connecting…")
        io.execute {
            try {
                val started = startForward(req)
                forward = started
                log("started ${started.listenAddr()} state=${started.state()}")
            } catch (e: MobileException.NeedsTrust) {
                // 首次连接：把指纹显示出来让用户和 `ssh` 命令行比对，确认后填进输入框再启动。
                log("需要确认主机指纹：${e.fingerprint}")
            } catch (e: MobileException.FingerprintMismatch) {
                log("指纹不一致，拒绝连接（记录 ${e.trusted}，实测 ${e.observed}）")
            } catch (e: Throwable) {
                log("失败：${e.message}")
            }
        }
    }

    private fun refresh() {
        val f = forward ?: run {
            log("还没有在跑的转发")
            return
        }
        val t = f.traffic()
        log("state=${f.state()} listen=${f.listenAddr()} up=${t.up} down=${t.down} lastError=${f.lastError()}")
    }

    private fun stop() {
        forward?.stop()
        log("stop 已发出")
    }

    private fun log(message: String) {
        Log.i(TAG, message)
        runOnUiThread { output.append("$message\n") }
    }

    override fun onDestroy() {
        forward?.stop()
        io.shutdown()
        super.onDestroy()
    }

    private companion object {
        const val TAG = "Threadline"
    }
}
